//! Credential-shaped strings in a store (ctx-hub-sync t-017).
//!
//! `ctx secrets scan` (`GET /api/secrets/scan`) looks for them in what a
//! store would sync, so they can be forgotten before its first push. Sync
//! will carry everything on a namespace's branches except raw session turns,
//! which stay local, so a scan covers that and skips turns unless asked.
//!
//! A finding names the path and the rule and shows a masked match, never the
//! value. The push guard (t-007) is meant to reuse [`scan_text`]: high
//! confidence blocks a push, medium only warns.
//!
//! Forgetting a path removes it from the branch's current state; the commits
//! before keep it. That's enough for sync, whose first push is shallow.

use std::collections::HashMap;
use std::sync::LazyLock;

use agentstategraph::{RepoError, Repository};
use agentstategraph_core::{Node, Object, ObjectId};
use regex::Regex;
use serde::Serialize;

/// How sure a rule is that a match is a real credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// A provider's own token format, or a private key.
    High,
    /// A shape secrets often take, which ordinary text sometimes takes too.
    Medium,
}

/// What a finding shows of its match.
#[derive(Clone, Copy)]
enum Reveal {
    /// The first `n` characters, then the length.
    Prefix(usize),
    /// All of it: the match isn't secret itself (a private key's header).
    Whole,
    /// The match with its first capture group masked (a key=value pair, a
    /// URL's password).
    Group,
}

pub struct Rule {
    pub id: &'static str,
    pub name: &'static str,
    pub confidence: Confidence,
    reveal: Reveal,
    pattern: &'static str,
}

/// Most specific first: a match that overlaps an earlier rule's is skipped,
/// so `sk-ant-…` is reported once, as Anthropic's, not also as OpenAI's.
const RULES: &[Rule] = &[
    Rule {
        id: "private-key",
        name: "Private key",
        confidence: Confidence::High,
        reveal: Reveal::Whole,
        pattern: r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----",
    },
    Rule {
        id: "aws-access-key",
        name: "AWS access key id",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(4),
        pattern: r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
    },
    Rule {
        id: "github-token",
        name: "GitHub token",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(4),
        pattern: r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{60,})\b",
    },
    Rule {
        id: "gitlab-token",
        name: "GitLab token",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(6),
        pattern: r"\bgl(?:pat|dt|rt|ptt|cbt)-[A-Za-z0-9_-]{20,}",
    },
    Rule {
        id: "anthropic-key",
        name: "Anthropic API key",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(7),
        pattern: r"\bsk-ant-[A-Za-z0-9_-]{20,}",
    },
    Rule {
        id: "openai-key",
        name: "OpenAI API key",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(3),
        pattern: r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,}",
    },
    Rule {
        id: "slack-token",
        name: "Slack token",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(5),
        pattern: r"\bxox[abposr]-[A-Za-z0-9-]{10,}",
    },
    Rule {
        id: "slack-webhook",
        name: "Slack webhook URL",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(32),
        pattern: r"https://hooks\.slack\.com/services/T[A-Za-z0-9_]+/B[A-Za-z0-9_]+/[A-Za-z0-9_]+",
    },
    Rule {
        id: "stripe-key",
        name: "Stripe live key",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(8),
        pattern: r"\b(?:sk|rk)_live_[A-Za-z0-9]{20,}",
    },
    Rule {
        id: "google-api-key",
        name: "Google API key",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(4),
        pattern: r"\bAIza[0-9A-Za-z_-]{35}",
    },
    Rule {
        id: "huggingface-token",
        name: "Hugging Face token",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(3),
        pattern: r"\bhf_[A-Za-z0-9]{34,}\b",
    },
    Rule {
        id: "npm-token",
        name: "npm token",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(4),
        pattern: r"\bnpm_[A-Za-z0-9]{36}\b",
    },
    Rule {
        id: "pypi-token",
        name: "PyPI token",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(5),
        pattern: r"\bpypi-AgEIcHlwaS5vcmc[A-Za-z0-9_-]{50,}",
    },
    Rule {
        id: "ctxone-token",
        name: "CTXone Hub token",
        confidence: Confidence::High,
        reveal: Reveal::Prefix(4),
        pattern: r"\bctx_[0-9a-f]{64}\b",
    },
    Rule {
        id: "jwt",
        name: "JSON web token",
        confidence: Confidence::Medium,
        reveal: Reveal::Prefix(3),
        pattern: r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
    },
    Rule {
        id: "url-password",
        name: "Password in a URL",
        confidence: Confidence::Medium,
        reveal: Reveal::Group,
        pattern: r"\b[a-zA-Z][a-zA-Z0-9+.-]{1,20}://[^\s/:@]{1,64}:([^\s/@]{3,128})@",
    },
    Rule {
        id: "assignment",
        name: "Secret-named value",
        confidence: Confidence::Medium,
        reveal: Reveal::Group,
        pattern: r#"(?i)\b(?:password|passwd|secret|client[_-]?secret|api[_-]?key|access[_-]?key|auth[_-]?token|token)["']?\s*[:=]\s*["']?([^\s"',;]{8,})"#,
    },
];

static COMPILED: LazyLock<Vec<(&'static Rule, Regex)>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|r| {
            (
                r,
                Regex::new(r.pattern).expect("secret rule patterns are valid"),
            )
        })
        .collect()
});

/// One credential-shaped match in a string.
#[derive(Debug)]
pub struct Hit {
    pub rule: &'static Rule,
    /// What can be shown of it; never the secret part.
    pub masked: String,
}

impl std::fmt::Debug for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id)
    }
}

/// Every credential-shaped match in `text`, most specific rule first.
pub fn scan_text(text: &str) -> Vec<Hit> {
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut hits = Vec::new();
    for (rule, re) in COMPILED.iter() {
        for caps in re.captures_iter(text) {
            let whole = caps.get(0).expect("group 0 is the match");
            let (start, end) = (whole.start(), whole.end());
            if taken.iter().any(|&(s, e)| start < e && s < end) {
                continue;
            }
            let secret = match rule.reveal {
                Reveal::Group => caps.get(1).map_or(whole.as_str(), |g| g.as_str()),
                _ => whole.as_str(),
            };
            if rule.reveal_is_secret() && is_placeholder(secret) {
                continue;
            }
            if rule.id == "assignment" && !has_letter_and_digit(secret) {
                continue;
            }
            taken.push((start, end));
            hits.push(Hit {
                rule,
                masked: mask(rule, &caps),
            });
        }
    }
    hits
}

impl Rule {
    fn reveal_is_secret(&self) -> bool {
        !matches!(self.reveal, Reveal::Whole)
    }
}

fn mask(rule: &Rule, caps: &regex::Captures<'_>) -> String {
    let whole = caps.get(0).expect("group 0 is the match");
    match rule.reveal {
        Reveal::Whole => whole.as_str().to_string(),
        Reveal::Prefix(n) => {
            let s = whole.as_str();
            let shown: String = s.chars().take(n).collect();
            format!("{shown}…({} chars)", s.chars().count())
        }
        Reveal::Group => match caps.get(1) {
            Some(g) => {
                let before = &whole.as_str()[..g.start() - whole.start()];
                let after = &whole.as_str()[g.end() - whole.start()..];
                format!("{before}…({} chars){after}", g.as_str().chars().count())
            }
            None => format!("…({} chars)", whole.as_str().chars().count()),
        },
    }
}

/// Documentation's stand-ins, which aren't credentials: AWS's `AKIA…EXAMPLE`,
/// `<token>`, `${API_KEY}`, `xxxxxxxx`, `changeme`.
fn is_placeholder(secret: &str) -> bool {
    let lower = secret.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "example",
        "xxxx",
        "****",
        "redacted",
        "changeme",
        "placeholder",
        "your_",
        "your-",
        "<",
        ">",
        "${",
        "{{",
        "…",
        "...",
    ];
    // Whole words that stand in for a password: `user:password@host`.
    const WORDS: &[&str] = &[
        "password", "passwd", "pass", "secret", "pwd", "token", "apikey",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) || WORDS.contains(&lower.as_str()) {
        return true;
    }
    if lower.starts_with('$') || lower.starts_with('/') || lower.starts_with('~') {
        return true;
    }
    let mut chars = secret.chars();
    let first = chars.next();
    first.is_some_and(|c| chars.all(|d| d == c))
}

fn has_letter_and_digit(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_alphabetic()) && s.chars().any(|c| c.is_ascii_digit())
}

/// A credential-shaped string at a path on a branch.
#[derive(Debug, Serialize)]
pub struct Finding {
    pub namespace: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub path: String,
    pub rule: &'static str,
    pub name: &'static str,
    pub confidence: Confidence,
    pub masked: String,
    /// The record to forget to remove it, where there is one: a memory entry
    /// or a part of a session's envelope. Elsewhere (a plan, say) the value
    /// is edited instead.
    pub forget: Option<String>,
}

#[derive(Default, Serialize)]
pub struct ScanReport {
    /// Strings checked, across every branch scanned.
    pub strings: usize,
    pub findings: Vec<Finding>,
    /// True when `findings` stopped at the limit.
    pub truncated: bool,
}

pub struct ScanOptions {
    /// Also scan raw session turns, which sync keeps local.
    pub turns: bool,
    /// Stop collecting findings after this many.
    pub max_findings: usize,
}

/// Subtrees read at once.
const BATCH: usize = 200;

/// A hit inside a scanned subtree, relative to the subtree's path.
#[derive(Clone)]
struct CachedHit {
    suffix: String,
    rule: &'static Rule,
    masked: String,
}

/// Scans branches, reading each subtree once. Branches share most of their
/// content, and a subtree's id names its content, so a subtree already
/// scanned on another branch, or in another namespace, reuses its hits
/// without being read again.
///
/// A branch is walked a node at a time down to each top-level key's children
/// and each session's parts; those are read whole.
pub struct Scanner {
    opts: ScanOptions,
    seen: HashMap<ObjectId, Vec<CachedHit>>,
    report: ScanReport,
}

impl Scanner {
    pub fn new(opts: ScanOptions) -> Self {
        Self {
            opts,
            seen: HashMap::new(),
            report: ScanReport::default(),
        }
    }

    pub fn truncated(&self) -> bool {
        self.report.truncated
    }

    pub fn finish(self) -> ScanReport {
        self.report
    }

    /// Scan one branch of `repo`'s namespace.
    pub fn scan_ref(&mut self, repo: &Repository, ref_name: &str) -> Result<(), RepoError> {
        let namespace = repo.namespace().as_str().to_string();
        for hit in self.node(repo, ref_name, "")? {
            if self.report.findings.len() >= self.opts.max_findings {
                self.report.truncated = true;
                break;
            }
            let path = if hit.suffix.is_empty() {
                "/".to_string()
            } else {
                hit.suffix
            };
            self.report.findings.push(Finding {
                namespace: namespace.clone(),
                ref_name: ref_name.to_string(),
                forget: forget_target(&path),
                path,
                rule: hit.rule.id,
                name: hit.rule.name,
                confidence: hit.rule.confidence,
                masked: hit.masked,
            });
        }
        Ok(())
    }

    /// The hits in the subtree at `path` ("" for the root), relative to it.
    fn node(
        &mut self,
        repo: &Repository,
        ref_name: &str,
        path: &str,
    ) -> Result<Vec<CachedHit>, RepoError> {
        let at = if path.is_empty() { "/" } else { path };
        let obj = repo.get(ref_name, at)?;
        let id = obj.id();
        if let Some(hits) = self.seen.get(&id) {
            return Ok(hits.clone());
        }
        let mut hits = Vec::new();
        if let Object::Node(Node::Map(children)) = &obj {
            let mut unread: Vec<(&str, ObjectId)> = Vec::new();
            for (key, child) in children {
                if !self.keeps(path, key) {
                    continue;
                }
                let prefix = format!("/{key}");
                text(&mut self.report.strings, &prefix, key, &mut hits);
                if let Some(cached) = self.seen.get(child) {
                    hits.extend(prefixed(cached, &prefix));
                } else if splits(&format!("{path}{prefix}")) {
                    let below = self.node(repo, ref_name, &format!("{path}{prefix}"))?;
                    hits.extend(prefixed(&below, &prefix));
                } else {
                    unread.push((key, *child));
                }
            }
            for chunk in unread.chunks(BATCH) {
                // The same content can sit at two keys: read it once.
                let mut fresh: Vec<(&str, ObjectId)> = Vec::new();
                for &(key, child) in chunk {
                    if !self.seen.contains_key(&child) && !fresh.iter().any(|(_, c)| *c == child) {
                        fresh.push((key, child));
                    }
                }
                let paths: Vec<String> = fresh
                    .iter()
                    .map(|(key, _)| format!("{path}/{key}"))
                    .collect();
                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                let values = if refs.is_empty() {
                    Vec::new()
                } else {
                    repo.get_json_many(ref_name, &refs)?
                };
                for ((_, child), value) in fresh.iter().zip(values) {
                    let mut below = Vec::new();
                    if let Some(value) = value {
                        let strings = &mut self.report.strings;
                        walk(&value, "", &mut |leaf, s| {
                            text(strings, leaf, s, &mut below)
                        });
                    }
                    self.seen.insert(*child, below);
                }
                for &(key, child) in chunk {
                    if let Some(below) = self.seen.get(&child) {
                        hits.extend(prefixed(below, &format!("/{key}")));
                    }
                }
            }
        } else {
            // A list, or a value, where a map usually is: read it whole.
            let value = repo.get_json(ref_name, at)?;
            let strings = &mut self.report.strings;
            walk(&value, "", &mut |leaf, s| text(strings, leaf, s, &mut hits));
        }
        self.seen.insert(id, hits.clone());
        Ok(hits)
    }

    /// Whether `key` under `path` is scanned: not /_meta, which is per-database
    /// schema bookkeeping and never syncs, and not a session's raw turns
    /// unless asked for.
    fn keeps(&self, path: &str, key: &str) -> bool {
        if path.is_empty() {
            return key != "_meta";
        }
        if key == "turns" && !self.opts.turns {
            return !is_session(path);
        }
        true
    }
}

/// `/sessions/<id>`.
fn is_session(path: &str) -> bool {
    path.strip_prefix("/sessions/")
        .is_some_and(|sid| !sid.is_empty() && !sid.contains('/'))
}

/// `/turns/<id>` on the turns ref: one session's raw turns.
fn is_session_turns(path: &str) -> bool {
    path.strip_prefix("/turns/")
        .is_some_and(|sid| !sid.is_empty() && !sid.contains('/'))
}

/// Whether the node at `path` is walked key by key rather than read whole:
/// each top-level key, each session, and each session's turns, which are
/// read a turn at a time.
fn splits(path: &str) -> bool {
    path.matches('/').count() == 1 || is_session(path) || is_session_turns(path)
}

fn prefixed(hits: &[CachedHit], prefix: &str) -> impl Iterator<Item = CachedHit> {
    hits.iter().map(move |h| CachedHit {
        suffix: format!("{prefix}{}", h.suffix),
        ..h.clone()
    })
}

/// Scan one string, recording its hits at `suffix`.
fn text(strings: &mut usize, suffix: &str, s: &str, hits: &mut Vec<CachedHit>) {
    *strings += 1;
    for hit in scan_text(s) {
        hits.push(CachedHit {
            suffix: suffix.to_string(),
            rule: hit.rule,
            masked: hit.masked,
        });
    }
}

/// Calls `f` with the path and text of every string in `value`, object keys
/// included (a pasted `.env` can land as keys).
fn walk(value: &serde_json::Value, path: &str, f: &mut impl FnMut(&str, &str)) {
    match value {
        serde_json::Value::String(s) => f(path, s),
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{path}/{i}"), f);
            }
        }
        serde_json::Value::Object(map) => {
            let base = path.trim_end_matches('/');
            for (key, item) in map {
                let child = format!("{base}/{key}");
                f(&child, key);
                walk(item, &child, f);
            }
        }
        _ => {}
    }
}

/// The record that holds `path`, to forget as a whole: a memory entry
/// (`/memory/<topic>/<id>`, or `/memory/{pinned,primed}/<source>/<section>`)
/// or one part of a session's envelope (`/sessions/<id>/<part>`).
pub(crate) fn forget_target(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["memory", kind @ ("pinned" | "primed"), source, section, ..] => {
            Some(format!("/memory/{kind}/{source}/{section}"))
        }
        ["memory", "pinned" | "primed", ..] => None,
        ["memory", topic, id, ..] => Some(format!("/memory/{topic}/{id}")),
        ["sessions", sid, part, ..] => Some(format!("/sessions/{sid}/{part}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Vec<&'static str> {
        scan_text(text).iter().map(|h| h.rule.id).collect()
    }

    // Test inputs are built at run time so the source holds no string that a
    // leak scanner would flag.
    fn repeat(c: char, n: usize) -> String {
        std::iter::repeat_n(c, n).collect()
    }
    fn mixed(n: usize) -> String {
        "a1B2c3D4e5F6g7H8i9J0".chars().cycle().take(n).collect()
    }

    #[test]
    fn each_provider_format_is_found() {
        let cases = [
            (format!("AKIA{}", "Q7ZR4M2KXW9PLT3B"), "aws-access-key"),
            (format!("ghp_{}", mixed(36)), "github-token"),
            (format!("glpat-{}", mixed(20)), "gitlab-token"),
            (format!("sk-ant-api03-{}", mixed(40)), "anthropic-key"),
            (format!("sk-proj-{}", mixed(40)), "openai-key"),
            (format!("xoxb-{}", mixed(24)), "slack-token"),
            (format!("sk_live_{}", mixed(24)), "stripe-key"),
            (format!("AIza{}", mixed(35)), "google-api-key"),
            (format!("hf_{}", mixed(34)), "huggingface-token"),
            (format!("npm_{}", mixed(36)), "npm-token"),
            (
                format!("ctx_{}", "0123456789abcdef".repeat(4)),
                "ctxone-token",
            ),
            (
                format!("-----BEGIN {}PRIVATE KEY-----", "OPENSSH "),
                "private-key",
            ),
        ];
        for (text, rule) in cases {
            assert_eq!(rules(&format!("the key is {text} today")), [rule], "{text}");
        }
    }

    #[test]
    fn a_match_is_reported_once_by_its_most_specific_rule() {
        let key = format!("sk-ant-api03-{}", mixed(48));
        assert_eq!(rules(&key), ["anthropic-key"]);
    }

    #[test]
    fn the_masked_form_never_holds_the_secret() {
        let token = format!("ghp_{}", mixed(36));
        let hits = scan_text(&format!("export GH={token}"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].masked, "ghp_…(40 chars)");
        assert!(!hits[0].masked.contains(&token[4..]));

        let pw = format!("hunter{}", 2042);
        let hits = scan_text(&format!("postgres://app:{pw}@db.internal:5432/app"));
        assert_eq!(hits[0].rule.id, "url-password");
        assert_eq!(hits[0].masked, "postgres://app:…(10 chars)@");

        let hits = scan_text(&format!("api_key = \"{}\"", mixed(24)));
        assert_eq!(hits[0].rule.id, "assignment");
        assert_eq!(hits[0].masked, "api_key = \"…(24 chars)");
    }

    #[test]
    fn documentation_stand_ins_are_not_findings() {
        let quiet = [
            format!("AKIA{}EXAMPLE", "IOSFODNN7"),
            format!("ghp_{}", repeat('x', 36)),
            "token: ${CTX_TOKEN}".to_string(),
            "password: <your password>".to_string(),
            "postgres://user:password@localhost/db".replace("password", "${PGPASSWORD}"),
            "api_key=changeme123".to_string(),
            "token_file = /run/secrets/hub_token".to_string(),
            "postgres://user:password@localhost/db".to_string(),
        ];
        for text in quiet {
            assert!(rules(&text).is_empty(), "{text}: {:?}", rules(&text));
        }
    }

    #[test]
    fn prose_about_tokens_is_not_a_finding() {
        for text in [
            "Rotate the token every 90 days; the token is read from a file.",
            "password: required for remote hubs",
            "The secret = something we keep out of git",
        ] {
            assert!(rules(text).is_empty(), "{text}: {:?}", rules(text));
        }
    }

    #[test]
    fn the_forget_target_is_the_whole_record() {
        assert_eq!(
            forget_target("/memory/security/18a9620f/fact").as_deref(),
            Some("/memory/security/18a9620f")
        );
        assert_eq!(
            forget_target("/memory/primed/runbook/deploy/body").as_deref(),
            Some("/memory/primed/runbook/deploy")
        );
        assert_eq!(
            forget_target("/sessions/abc/summary/0").as_deref(),
            Some("/sessions/abc/summary")
        );
        assert_eq!(forget_target("/plans/p/tasks/t-001/title"), None);
    }

    fn store() -> Repository {
        let repo = Repository::new(Box::new(agentstategraph_storage::MemoryStorage::new()));
        repo.init().expect("init");
        repo
    }

    fn put(repo: &Repository, ref_name: &str, path: &str, value: serde_json::Value) {
        let opts = agentstategraph::CommitOptions::new(
            "test",
            agentstategraph_core::IntentCategory::Custom("Observe".into()),
            "write",
        );
        repo.set_json(ref_name, path, &value, opts).expect("write");
    }

    fn scan(repo: &Repository, refs: &[&str], turns: bool) -> ScanReport {
        let mut scanner = Scanner::new(ScanOptions {
            turns,
            max_findings: 100,
        });
        for r in refs {
            scanner.scan_ref(repo, r).expect("scan");
        }
        scanner.finish()
    }

    /// A store with a key in a memory entry, one in a session's summary and
    /// one in that session's raw turns.
    fn seeded() -> Repository {
        let repo = store();
        let key = format!("ghp_{}", mixed(36));
        put(
            &repo,
            "main",
            "/memory/ci/f1",
            serde_json::json!({"fact": format!("CI uses {key}")}),
        );
        put(
            &repo,
            "main",
            "/memory/ci/f2",
            serde_json::json!({"fact": "nothing here"}),
        );
        put(
            &repo,
            "main",
            "/sessions/s1/summary",
            serde_json::json!(["set GH to the token", key]),
        );
        put(
            &repo,
            "main",
            "/sessions/s1/turns/t0000",
            serde_json::json!({"text": format!("my key is {key}")}),
        );
        put(
            &repo,
            "main",
            "/plans/p/title",
            serde_json::json!("ship it"),
        );
        repo
    }

    #[test]
    fn a_scan_covers_what_syncs_and_skips_raw_turns() {
        let report = scan(&seeded(), &["main"], false);
        let found: Vec<(&str, Option<&str>)> = report
            .findings
            .iter()
            .map(|f| (f.path.as_str(), f.forget.as_deref()))
            .collect();
        assert_eq!(
            found,
            [
                ("/memory/ci/f1/fact", Some("/memory/ci/f1")),
                ("/sessions/s1/summary/1", Some("/sessions/s1/summary")),
            ]
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.rule == "github-token" && f.ref_name == "main")
        );
        assert!(!report.truncated);
    }

    #[test]
    fn turns_are_scanned_when_asked_for() {
        let report = scan(&seeded(), &["main"], true);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.path == "/sessions/s1/turns/t0000/text")
        );
        assert_eq!(report.findings.len(), 3);
    }

    #[test]
    fn turns_on_the_turns_ref_are_scanned_a_turn_at_a_time() {
        let repo = store();
        let key = format!("ghp_{}", mixed(36));
        for idx in 0..3 {
            let text = if idx == 1 {
                format!("my key is {key}")
            } else {
                "nothing here".to_string()
            };
            crate::turns::write_turn(
                &repo,
                "s1",
                idx,
                &serde_json::json!({ "text": text }),
                agentstategraph::CommitOptions::new(
                    "test",
                    agentstategraph_core::IntentCategory::Custom("Observe".into()),
                    "turn",
                ),
            )
            .expect("write turn");
        }
        let report = scan(&repo, &[crate::turns::TURNS_REF], true);
        let found: Vec<(&str, Option<&str>)> = report
            .findings
            .iter()
            .map(|f| (f.path.as_str(), f.forget.as_deref()))
            .collect();
        assert_eq!(found, [("/turns/s1/t0001/text", None)]);
        assert!(
            splits("/turns/s1"),
            "a session's turns are read a turn at a time"
        );
    }

    #[test]
    fn a_branch_that_shares_content_is_not_read_again() {
        let repo = seeded();
        repo.branch("feat", "main").expect("branch");
        let one = scan(&repo, &["main"], false);
        let both = scan(&repo, &["main", "feat"], false);
        // Every finding on both branches, each read once.
        assert_eq!(both.findings.len(), 2 * one.findings.len());
        assert!(both.findings.iter().any(|f| f.ref_name == "feat"));
        assert_eq!(both.strings, one.strings);

        // After a change on feat, its scan reads the keys on the way down
        // (3 at the root, 1 under /memory) and the topic that changed
        // (/memory/ci: 2 entries of a key, a field name and a value), but no
        // session and no plan.
        put(
            &repo,
            "feat",
            "/memory/ci/f2",
            serde_json::json!({"fact": format!("AKIA{}", "Q7ZR4M2KXW9PLT3B")}),
        );
        let changed = scan(&repo, &["main", "feat"], false);
        assert!(
            changed
                .findings
                .iter()
                .any(|f| f.ref_name == "feat" && f.rule == "aws-access-key")
        );
        assert_eq!(changed.strings, one.strings + 3 + 1 + 6);
    }

    #[test]
    fn findings_stop_at_the_limit() {
        let repo = seeded();
        let mut scanner = Scanner::new(ScanOptions {
            turns: true,
            max_findings: 2,
        });
        scanner.scan_ref(&repo, "main").expect("scan");
        assert!(scanner.truncated());
        assert_eq!(scanner.finish().findings.len(), 2);
    }
}
