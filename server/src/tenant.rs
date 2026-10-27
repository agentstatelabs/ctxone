//! Tenant identity for a multi-tenant Hub: which tenant a request's bearer
//! token belongs to (docs/design/MULTI_TENANT_HUB.md, "Identity").
//!
//! A [`TenantResolver`] maps a token to a [`Tenant`]. The Hub ships one, the
//! [`TenantsFile`] (`--tenants-file`): lines of `<tenant> <sha256 of a token>`,
//! re-read every second so tenants and tokens change without a restart. An
//! account service can supply its own resolver. `ctxone-hub tenant` edits the
//! file ([`crate::tenant_admin`]).
//!
//! The auth middleware in [`crate::http`] resolves every request's token and
//! attaches the `Tenant` to the request, so nothing downstream chooses one.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tracing::warn;

/// The longest tenant name: a DNS label's limit, so a name can become a
/// hostname later.
pub const TENANT_NAME_MAX: usize = 63;

/// A tenant's name: 1 to 63 of `a-z`, `0-9` and `-`. It's the AgentStateGraph
/// `tenant_id` of the tenant's data and the `tenant_id` of its `ctxone_*`
/// rows. Cheap to clone.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tenant(Arc<str>);

/// A tenant name that isn't `[a-z0-9-]{1,63}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tenant name {name:?} isn't 1 to 63 of a-z, 0-9 and -")]
pub struct InvalidTenantName {
    pub name: String,
}

impl Tenant {
    pub fn new(name: &str) -> Result<Self, InvalidTenantName> {
        let valid = (1..=TENANT_NAME_MAX).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if valid {
            Ok(Self(name.into()))
        } else {
            Err(InvalidTenantName {
                name: name.to_string(),
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Tenant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The SHA-256 of a bearer token. A tenants file stores these instead of the
/// tokens, so reading the file doesn't reveal one.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    pub fn of(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }

    /// Parse 64 hex digits, in either case.
    pub fn from_hex(hex: &str) -> Option<Self> {
        // Hex digits only: `from_str_radix` alone would take a `+` sign.
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut out = [0u8; 32];
        for (byte, pair) in out.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let pair = std::str::from_utf8(pair).ok()?;
            *byte = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(out))
    }

    /// Lowercase hex, the form a tenants file holds.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Compare without stopping at the first differing byte.
    fn ct_eq(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
    }
}

impl std::fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenHash(..)")
    }
}

/// A resolver couldn't answer, for instance because the account service it
/// asks is down. The request gets a 503, not a 401. The message is logged, so
/// it must never contain the token.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ResolveError(pub String);

/// What [`TenantResolver::resolve`] returns.
pub type Resolution<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Tenant>, ResolveError>> + Send + 'a>>;

/// Maps a bearer token to its tenant. The seam for an account service; the
/// Hub's own is [`TenantsFile`].
pub trait TenantResolver: Send + Sync + std::fmt::Debug {
    /// The tenant `token` belongs to, or `None` when no tenant holds it (the
    /// request gets a 401).
    fn resolve<'a>(&'a self, token: &'a str) -> Resolution<'a>;

    /// Whether `tenant` still holds a token this resolver accepts. The Hub asks
    /// about each open tenant every upkeep pass and closes one that doesn't,
    /// after flushing it, so a disabled or deleted tenant leaves the Hub's
    /// memory within a pass. A resolver that can't tell answers `true`, and its
    /// tenants close when they're idle.
    fn admits(&self, tenant: &Tenant) -> bool {
        let _ = tenant;
        true
    }
}

/// How often a [`TenantsFile`] is re-read.
const TENANTS_FILE_RELOAD: std::time::Duration = std::time::Duration::from_secs(1);

/// Tenants and their tokens, read from a file of `<tenant> <sha256 of a
/// token>` lines. A tenant may have several lines (one per device, say), and
/// blank lines and `#` comments are ignored. Only hashes are stored, so a
/// token is shown once, when it's made. A line `disabled <tenant> <sha256 of a
/// token>` keeps a token and refuses it, which is how `ctxone-hub tenant
/// disable` turns a tenant away and keeps its data.
///
/// The file is re-read at most once a second, so adding, rotating or removing
/// a tenant needs no restart. Opening refuses a file with a bad line. A later
/// read skips bad lines with a warning and uses the rest, so a typo locks out
/// only its own line and never keeps a removed token alive. A hash listed
/// under two tenants would give one token both, so neither gets it. If a later
/// read fails outright, the tenants last read stay in force and a warning is
/// logged; to refuse every token, empty the file rather than delete it.
#[derive(Clone)]
pub struct TenantsFile {
    inner: Arc<TenantsFileInner>,
}

struct TenantsFileInner {
    path: PathBuf,
    reload: std::time::Duration,
    state: std::sync::Mutex<TenantsFileState>,
}

struct TenantsFileState {
    entries: Arc<[Entry]>,
    problems: Vec<LineProblem>,
    read_at: std::time::Instant,
}

/// A line of a tenants file that holds a token.
#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) hash: TokenHash,
    pub(crate) tenant: Tenant,
    /// A `disabled` line: the token is kept, and refused.
    pub(crate) disabled: bool,
}

/// A line of a tenants file that can't be used, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineProblem {
    pub line: usize,
    pub reason: String,
}

impl std::fmt::Display for LineProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

/// Why a tenants file couldn't be opened.
#[derive(Debug, thiserror::Error)]
pub enum TenantsFileError {
    #[error("can't read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {}", path.display(), problems.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid {
        path: PathBuf,
        problems: Vec<LineProblem>,
    },
}

impl std::fmt::Debug for TenantsFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The path only, never the hashes.
        f.debug_struct("TenantsFile")
            .field("path", &self.inner.path)
            .finish_non_exhaustive()
    }
}

impl TenantsFile {
    /// Read `path` now, failing if it can't be read or has a bad line, and
    /// re-read it at most once a second from then on.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, TenantsFileError> {
        Self::open_with_reload(path, TENANTS_FILE_RELOAD)
    }

    /// [`open`](Self::open) with another re-read interval (tests use zero).
    pub fn open_with_reload(
        path: impl Into<PathBuf>,
        reload: std::time::Duration,
    ) -> Result<Self, TenantsFileError> {
        let path = path.into();
        let text = read(&path)?;
        let (entries, problems) = parse(&text);
        if !problems.is_empty() {
            return Err(TenantsFileError::Invalid { path, problems });
        }
        Ok(Self {
            inner: Arc::new(TenantsFileInner {
                path,
                reload,
                state: std::sync::Mutex::new(TenantsFileState {
                    entries: entries.into(),
                    problems,
                    read_at: std::time::Instant::now(),
                }),
            }),
        })
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The tenant `token` belongs to, re-reading the file when it's due. The
    /// token's hash is compared with every listed hash in constant time.
    pub fn lookup(&self, token: &str) -> Option<Tenant> {
        if token.is_empty() {
            return None;
        }
        let presented = TokenHash::of(token);
        let mut found = None;
        for entry in self.entries().iter() {
            // Every hash is compared, disabled or not, so the time taken
            // doesn't tell a disabled token from an unknown one.
            if entry.hash.ct_eq(&presented) && !entry.disabled {
                found = Some(entry.tenant.clone());
            }
        }
        found
    }

    /// How many tenants hold a token the file accepts, and how many such
    /// tokens there are. Disabled tokens aren't counted.
    pub fn counts(&self) -> (usize, usize) {
        let entries = self.entries();
        let accepted: Vec<&Entry> = entries.iter().filter(|e| !e.disabled).collect();
        let mut tenants: Vec<&Tenant> = accepted.iter().map(|e| &e.tenant).collect();
        tenants.sort();
        tenants.dedup();
        (tenants.len(), accepted.len())
    }

    /// Whether `tenant` holds a token the file accepts.
    pub fn admits(&self, tenant: &Tenant) -> bool {
        self.entries()
            .iter()
            .any(|e| !e.disabled && e.tenant == *tenant)
    }

    fn entries(&self) -> Arc<[Entry]> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.read_at.elapsed() >= self.inner.reload {
            state.read_at = std::time::Instant::now();
            match read(&self.inner.path) {
                Ok(text) => {
                    let (entries, problems) = parse(&text);
                    // Once per change, not on every re-read.
                    if !problems.is_empty() && problems != state.problems {
                        warn!(
                            path = %self.inner.path.display(),
                            problems = %problems
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join("; "),
                            "tenants file has lines that can't be used; skipping them"
                        );
                    }
                    state.entries = entries.into();
                    state.problems = problems;
                }
                Err(e) => warn!(
                    error = %e,
                    "tenants file unreadable; keeping the tenants last read"
                ),
            }
        }
        state.entries.clone()
    }
}

impl TenantResolver for TenantsFile {
    fn resolve<'a>(&'a self, token: &'a str) -> Resolution<'a> {
        Box::pin(std::future::ready(Ok(self.lookup(token))))
    }

    fn admits(&self, tenant: &Tenant) -> bool {
        TenantsFile::admits(self, tenant)
    }
}

fn read(path: &Path) -> Result<String, TenantsFileError> {
    std::fs::read_to_string(path).map_err(|source| TenantsFileError::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// The word that starts a disabled token's line.
pub(crate) const DISABLED: &str = "disabled";

/// Read one line of a tenants file: `None` for a blank line or a comment,
/// otherwise its token or why the line can't be used.
pub(crate) fn parse_line(raw: &str) -> Option<Result<Entry, String>> {
    let content = raw.trim();
    if content.is_empty() || content.starts_with('#') {
        return None;
    }
    let fields: Vec<&str> = content.split_whitespace().collect();
    let (name, hash, disabled) = match fields[..] {
        [name, hash] => (name, hash, false),
        [DISABLED, name, hash] => (name, hash, true),
        _ => {
            return Some(Err(
                "expected `<tenant> <sha256 of a token>` or `disabled <tenant> <sha256 of a token>`"
                    .into(),
            ));
        }
    };
    let tenant = match Tenant::new(name) {
        Ok(t) => t,
        Err(e) => return Some(Err(e.to_string())),
    };
    let Some(hash) = TokenHash::from_hex(hash) else {
        return Some(Err("the token hash isn't 64 hex digits (a SHA-256)".into()));
    };
    Some(Ok(Entry {
        hash,
        tenant,
        disabled,
    }))
}

/// The usable entries of a tenants file, and the lines that aren't.
pub(crate) fn parse(text: &str) -> (Vec<Entry>, Vec<LineProblem>) {
    let mut lines: Vec<(usize, Entry)> = Vec::new();
    let mut problems = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        match parse_line(raw) {
            None => {}
            Some(Ok(entry)) => lines.push((line, entry)),
            Some(Err(reason)) => problems.push(LineProblem { line, reason }),
        }
    }

    // A hash listed for two tenants would give one token both: refuse it for
    // each, whether or not either line is disabled. The same hash twice for
    // one tenant is harmless; keep one, refused if either line is disabled.
    let mut tenants_by_hash: HashMap<TokenHash, Vec<(usize, &Tenant)>> = HashMap::new();
    for (line, entry) in &lines {
        tenants_by_hash
            .entry(entry.hash)
            .or_default()
            .push((*line, &entry.tenant));
    }
    let mut ambiguous = Vec::new();
    for listed in tenants_by_hash.values() {
        if listed.iter().any(|(_, t)| *t != listed[0].1) {
            for (line, _) in listed {
                ambiguous.push(LineProblem {
                    line: *line,
                    reason: format!(
                        "the same token hash is listed for tenants {}",
                        listed
                            .iter()
                            .map(|(l, t)| format!("{t} (line {l})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }
    }
    let refused: std::collections::HashSet<usize> = ambiguous.iter().map(|p| p.line).collect();
    problems.extend(ambiguous);
    problems.sort_by_key(|p| p.line);

    let mut entries: Vec<Entry> = Vec::new();
    let mut index: HashMap<TokenHash, usize> = HashMap::new();
    for (line, entry) in lines {
        if refused.contains(&line) {
            continue;
        }
        match index.get(&entry.hash) {
            Some(&i) => entries[i].disabled |= entry.disabled,
            None => {
                index.insert(entry.hash, entries.len());
                entries.push(entry);
            }
        }
    }
    (entries, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(token: &str) -> String {
        TokenHash::of(token).to_hex()
    }

    #[test]
    fn tenant_names_are_lowercase_alphanumerics_and_hyphens() {
        for ok in ["a", "alice", "team-7", "0", &"x".repeat(63)] {
            assert!(Tenant::new(ok).is_ok(), "{ok:?} should be valid");
        }
        for bad in ["", "Alice", "a_b", "a.b", "a b", "ü", &"x".repeat(64)] {
            assert!(Tenant::new(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn token_hash_round_trips_hex_in_either_case() {
        let h = TokenHash::of("s3cret");
        assert_eq!(h.to_hex().len(), 64);
        assert_eq!(TokenHash::from_hex(&h.to_hex()), Some(h));
        assert_eq!(TokenHash::from_hex(&h.to_hex().to_uppercase()), Some(h));
        assert_eq!(TokenHash::from_hex(&h.to_hex()[..63]), None);
        assert_eq!(TokenHash::from_hex(&"g".repeat(64)), None);
        // A known vector, so the hash is plain SHA-256 of the token's bytes.
        assert_eq!(
            TokenHash::of("abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_reads_tenants_with_several_tokens() {
        let text = format!(
            "# tenants\n\nalice {}\nalice {}\n  bob   {}  \n",
            hash("a1"),
            hash("a2"),
            hash("b1")
        );
        let (entries, problems) = parse(&text);
        assert!(problems.is_empty(), "{problems:?}");
        let names: Vec<&str> = entries.iter().map(|e| e.tenant.as_str()).collect();
        assert_eq!(names, ["alice", "alice", "bob"]);
    }

    #[test]
    fn parse_reports_bad_lines_by_number_and_keeps_the_rest() {
        let text = format!(
            "alice {}\nBob {}\ncarol not-a-hash\ndave\nerin {} extra\nfrank {}\n",
            hash("a"),
            hash("b"),
            hash("e"),
            hash("f")
        );
        let (entries, problems) = parse(&text);
        let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, [2, 3, 4, 5]);
        let names: Vec<&str> = entries.iter().map(|e| e.tenant.as_str()).collect();
        assert_eq!(names, ["alice", "frank"]);
        // Problems never quote the hash.
        for p in &problems {
            assert!(!p.reason.contains(&hash("b")), "{p}");
        }
    }

    #[test]
    fn a_hash_listed_for_two_tenants_goes_to_neither() {
        let shared = hash("shared");
        let text = format!(
            "alice {shared}\nbob {shared}\nalice {}\nalice {}\n",
            hash("a"),
            hash("a")
        );
        let (entries, problems) = parse(&text);
        let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, [1, 2]);
        assert!(problems[0].reason.contains("alice (line 1), bob (line 2)"));
        // alice's own token, listed twice, is kept once.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tenant.as_str(), "alice");
    }

    fn file_with(text: &str) -> (tempfile::TempDir, TenantsFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tenants");
        std::fs::write(&path, text).unwrap();
        let file = TenantsFile::open(&path).unwrap();
        (dir, file)
    }

    #[test]
    fn a_disabled_line_keeps_its_token_and_refuses_it() {
        let text = format!(
            "alice {}\ndisabled bob {}\ncarol {}\ndisabled carol {}\n",
            hash("a"),
            hash("b"),
            hash("c1"),
            hash("c2")
        );
        let (_dir, file) = file_with(&text);
        assert_eq!(file.lookup("a").unwrap().as_str(), "alice");
        assert_eq!(file.lookup("b"), None);
        assert_eq!(file.lookup("c1").unwrap().as_str(), "carol");
        assert_eq!(file.lookup("c2"), None);
        // Accepted tenants and tokens only.
        assert_eq!(file.counts(), (2, 2));
        let admits = |name| file.admits(&Tenant::new(name).unwrap());
        assert!(admits("alice"));
        assert!(!admits("bob"));
        assert!(admits("carol"));
        assert!(!admits("dave"));
    }

    #[test]
    fn a_hash_shared_with_a_disabled_line_goes_to_neither() {
        let shared = hash("shared");
        let text = format!("alice {shared}\ndisabled bob {shared}\n");
        let (entries, problems) = parse(&text);
        let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, [1, 2]);
        assert!(entries.is_empty());
    }

    #[test]
    fn a_token_listed_twice_is_refused_if_either_line_is_disabled() {
        let text = format!("alice {}\ndisabled alice {}\n", hash("a"), hash("a"));
        let (entries, problems) = parse(&text);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].disabled);
    }

    #[test]
    fn disabled_lines_are_checked_like_the_others() {
        let text = format!(
            "disabled Bob {}\ndisabled carol nope\ndisabled dave\ndisabled erin {} more\n",
            hash("b"),
            hash("e")
        );
        let (entries, problems) = parse(&text);
        let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
        assert_eq!(lines, [1, 2, 3, 4]);
        assert!(entries.is_empty());
        // `disabled` alone is a tenant name like any other.
        let (entries, problems) = parse(&format!("disabled {}\n", hash("d")));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(entries[0].tenant.as_str(), "disabled");
        assert!(!entries[0].disabled);
    }
}
