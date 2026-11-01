//! Routing a transcript to the workspace (namespace) it was recorded in.
//!
//! Sessions used to land wherever the *invocation* pointed: `ctx
//! ingest-session --all` walks every project on the machine, but the hub
//! passed a single `--namespace` for the whole run, so transcripts from N
//! repos collapsed into one workspace (in practice `default`).
//!
//! The fix is to route per transcript rather than per run. Every source
//! records the working directory the session ran in — Claude Code on each
//! entry, Codex in `session_meta` — and that `cwd` resolves through the
//! project registry to a namespace.
//!
//! Two things make this cheap enough to run over a whole machine:
//!
//! - **Memoized by cwd, not by session.** ~350 transcripts share a few dozen
//!   directories, so detection runs a few dozen times, not 350.
//! - **One client per namespace.** `Cli::http_client` already accepts a
//!   namespace and bakes it into a default header, so a routed write is just
//!   a different client — no per-request URL rewriting and no change to any
//!   of the `store_*` writers.
//!
//! Unresolvable sessions (no cwd recorded, no git root) fall back to the
//! default namespace. That is the honest answer: guessing a workspace from a
//! lossy directory label is how the old `label`-based grouping produced wrong
//! answers.
//!
//! A directory whose detection *failed* (hub timed out, errored, unreachable)
//! is different: the hub never said it has no project, so its sessions are
//! skipped this run ([`Routed::Unresolved`]) rather than filed in `default` or
//! registered as a second project, and the next sync picks them up.

use std::collections::HashMap;

/// Outcome of routing one transcript, kept for the run summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed {
    /// Resolved to an existing registered project.
    Existing(String),
    /// Registered a new project (and namespace) for this repo.
    Registered(String),
    /// Would have registered this repo, but the run is a dry run. Reported so
    /// `--dry-run` still answers "where would my sessions land", without
    /// creating the namespace as a side effect of asking.
    WouldRegister(String),
    /// No workspace could be determined; the default namespace is used.
    Fallback,
    /// Detection failed for this directory, so nobody knows where it belongs.
    /// Not a destination: sessions routed here are skipped, not written.
    Unresolved(String),
}

impl Routed {
    /// The namespace to write into, or `None` for the default.
    ///
    /// `WouldRegister` maps to `None`: the namespace does not exist yet, so a
    /// real write against it would 404. `Unresolved` also maps to `None`, but
    /// is not a write target at all — check [`Self::unresolved`] first.
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Routed::Existing(ns) | Routed::Registered(ns) => Some(ns.as_str()),
            Routed::WouldRegister(_) | Routed::Fallback | Routed::Unresolved(_) => None,
        }
    }

    /// Why detection failed, when it did.
    pub fn unresolved(&self) -> Option<&str> {
        match self {
            Routed::Unresolved(reason) => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// Resolves `cwd` → namespace, memoized for the lifetime of one ingest run.
pub struct Router {
    server: String,
    probe: reqwest::Client,
    auto_register: bool,
    /// A dry run resolves and reports but never registers — creating a
    /// namespace is a write, and `--dry-run` promises none.
    dry_run: bool,
    /// Where a transcript with no resolvable cwd goes.
    ///
    /// For `--all` this is `default`: the machine-wide sync fans across repos
    /// and unroutable transcripts should not scatter. For a single `--file` or
    /// `--namespace X` run it is that namespace, so an explicit target is
    /// honoured instead of being silently overridden to `default` — the bug
    /// that let a `--file` re-ingest land a duplicate in `default`.
    fallback: Option<String>,
    /// When set, a working directory that is **not** a git repo still becomes a
    /// workspace — keyed on the directory's own basename — instead of dropping
    /// to the fallback/`default`. Opt-in (`ctx ingest-session --dir-workspaces`)
    /// because bare directory names are less canonical than git-remote identity:
    /// they can collide (`/a/app` and `/b/app`) and proliferate. Git repos still
    /// prefer their remote's `host/owner/repo` identity.
    dir_workspaces: bool,
    /// cwd → routing outcome. Includes negative results so a directory that
    /// cannot be resolved is not re-probed once per transcript.
    memo: HashMap<String, Routed>,
    /// namespace → client. `None` key is the default-namespace client.
    clients: HashMap<Option<String>, reqwest::Client>,
    /// namespace → tombstoned session ids, fetched on first use of that
    /// workspace. Tombstones are namespace-scoped, so a session is checked
    /// against the workspace it actually routes to.
    tombstones: HashMap<Option<String>, std::collections::HashSet<String>>,
}

impl Router {
    pub fn new(server: &str, auto_register: bool, dry_run: bool, fallback: Option<String>) -> Self {
        Self {
            server: server.to_string(),
            dry_run,
            fallback,
            // Bounded, so a stuck hub cannot stall the sync; long enough for
            // the hub's own detect timeout to answer first and explain itself.
            // A miss skips one directory's sessions for this run.
            probe: reqwest::Client::builder()
                .timeout(crate::detect::DETECT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            auto_register,
            dir_workspaces: false,
            memo: HashMap::new(),
            clients: HashMap::new(),
            tombstones: HashMap::new(),
        }
    }

    /// Enable directory-as-workspace routing (see [`Self::dir_workspaces`]).
    pub fn with_dir_workspaces(mut self, on: bool) -> Self {
        self.dir_workspaces = on;
        self
    }

    /// The repo/workspace root for a cwd: the git root when the directory is in
    /// a repo, else — only under `--dir-workspaces` — the directory itself, so a
    /// non-git working directory keys a workspace on its own name.
    fn resolve_root(&self, dir: &std::path::Path) -> Option<std::path::PathBuf> {
        match crate::find_git_root(dir) {
            Some(root) => Some(root),
            None if self.dir_workspaces => Some(dir.to_path_buf()),
            None => None,
        }
    }

    /// Whether this session was deleted and must not be re-imported.
    ///
    /// The transcript of a deleted session is still on disk, so without this
    /// the next sync would faithfully restore what the user asked to remove.
    pub async fn is_tombstoned(&mut self, ns: Option<&str>, sid: &str) -> bool {
        let key = ns.map(str::to_string);
        if !self.tombstones.contains_key(&key) {
            let set = fetch_tombstones(&self.server, &self.probe, ns).await;
            self.tombstones.insert(key.clone(), set);
        }
        self.tombstones
            .get(&key)
            .map(|s| s.contains(sid))
            .unwrap_or(false)
    }

    /// Route one transcript. `cwd` is whatever the source recorded.
    pub async fn route(&mut self, cwd: Option<&str>) -> Routed {
        // No cwd (e.g. an explicit `--file`) → the configured fallback, which
        // for a targeted run is the caller's `--namespace`. Returning a bare
        // `Fallback` here is what silently overrode `--namespace` to `default`.
        let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
            return self.fallback();
        };
        if let Some(hit) = self.memo.get(cwd) {
            return hit.clone();
        }
        let outcome = self.resolve(cwd).await;
        self.memo.insert(cwd.to_string(), outcome.clone());
        outcome
    }

    async fn resolve(&self, cwd: &str) -> Routed {
        match self.detect(cwd).await {
            Ok(Some(ns)) => return Routed::Existing(ns),
            Ok(None) => {}
            // Never register on a failed detection: the repo may already be a
            // project under a different id, and minting one from its remote
            // would split it across two workspaces.
            Err(reason) => return Routed::Unresolved(reason),
        }
        if !self.auto_register {
            return self.fallback();
        }
        if self.dry_run {
            // Work out the id without calling the registry, so a dry run can
            // show the intended workspace while staying read-only.
            return match self.would_register(cwd) {
                Some(id) => Routed::WouldRegister(id),
                None => self.fallback(),
            };
        }
        match self.register(cwd).await {
            Some(ns) => Routed::Registered(ns),
            None => self.fallback(),
        }
    }

    /// The configured fallback as a routing outcome. An explicit fallback is
    /// reported as `Existing` (it is a real namespace the caller named);
    /// absent, it is `Fallback`, which resolves to the default namespace.
    fn fallback(&self) -> Routed {
        match &self.fallback {
            Some(ns) => Routed::Existing(ns.clone()),
            None => Routed::Fallback,
        }
    }

    /// The id `register` would mint, computed locally (git only, no HTTP).
    fn would_register(&self, cwd: &str) -> Option<String> {
        let dir = std::path::Path::new(cwd);
        if !dir.is_dir() {
            return None;
        }
        let root = self.resolve_root(dir)?;
        project_id_for(&root, crate::read_git_remote(&root).as_deref())
    }

    /// Ask the hub to run its project-detection chain for this directory.
    /// `Ok(None)` is the hub saying "no project"; `Err` is it not saying.
    async fn detect(&self, cwd: &str) -> Result<Option<String>, String> {
        crate::detect::detect(&self.probe, &self.server, cwd)
            .await
            .map_err(|f| f.detail)
    }

    /// Mint a project for an unregistered repo, which is also the only path
    /// that creates a namespace (`POST /api/projects` calls `fork_namespace
    /// (ns).init()`). Keeping it as the sole creator is why `GET
    /// /api/projects` stays a truthful list of workspaces.
    ///
    /// Requires a git root: a directory that is not a repo has no stable
    /// identity to key a workspace on, so those stay in the default namespace.
    async fn register(&self, cwd: &str) -> Option<String> {
        let dir = std::path::Path::new(cwd);
        if !dir.is_dir() {
            return None; // deleted worktree or moved checkout
        }
        let root = self.resolve_root(dir)?;
        // A non-git directory has no remote; project_id_for then keys on the
        // directory basename.
        let remote = crate::read_git_remote(&root);
        let id = project_id_for(&root, remote.as_deref())?;

        let body = serde_json::json!({
            "id": id,
            "remote_url": remote,
            "local_path": root.to_string_lossy(),
            "display_name": root.file_name().map(|n| n.to_string_lossy().to_string()),
        });
        let resp = self
            .probe
            .post(format!("{}/api/projects", self.server))
            .json(&body)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = resp.json().await.ok()?;
        v["namespace"]
            .as_str()
            .or(Some(id.as_str()))
            .map(str::to_string)
    }

    /// A client whose default headers target `ns`, built once per namespace.
    pub fn client_for(
        &mut self,
        ns: Option<&str>,
        build: impl FnOnce(Option<&str>) -> reqwest::Client,
    ) -> reqwest::Client {
        let key = ns.map(str::to_string);
        self.clients.entry(key).or_insert_with(|| build(ns)).clone()
    }

    /// cwd → namespace decisions made this run, for the summary line.
    pub fn decisions(&self) -> Vec<(&str, &Routed)> {
        let mut out: Vec<(&str, &Routed)> =
            self.memo.iter().map(|(k, v)| (k.as_str(), v)).collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }
}

/// Session ids the hub has been told to forget, fetched once per run.
///
/// Read up-front rather than probed per session: a machine-wide sync
/// considers hundreds of transcripts, and the transcripts of deleted sessions
/// are still sitting on disk. Without this, "delete" would mean "delete until
/// the next sync".
///
/// A failure here returns an empty set — a hub too old to have the endpoint,
/// or briefly unreachable, must not silently start skipping sessions.
pub async fn fetch_tombstones(
    server: &str,
    client: &reqwest::Client,
    namespace: Option<&str>,
) -> std::collections::HashSet<String> {
    let mut url = format!("{server}/api/session_tombstones");
    if let Some(ns) = namespace {
        url.push_str(&format!("?namespace={}", crate::urlencoding(ns)));
    }
    let Ok(resp) = client.get(url).send().await else {
        return Default::default();
    };
    if !resp.status().is_success() {
        return Default::default();
    }
    let Ok(v) = resp.json::<serde_json::Value>().await else {
        return Default::default();
    };
    v["sessions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Derive a stable, kebab-case id for a new project.
///
/// Prefers the remote's whole canonical form, `host/owner/repo` (ctx-hub-sync
/// t-001), so the same repo cloned to two paths, checked out as a worktree, or
/// cloned over ssh and over https resolves to one workspace, and
/// `github.com/a/app` and `github.com/b/app` don't share one. A remote with no
/// canonical form, such as a local path, gives its last path segment, and a
/// repo with no remote its directory name.
///
/// Only new projects get these ids: a registered project keeps its id, and the
/// Hub finds it by its remote first.
pub fn project_id_for(root: &std::path::Path, remote: Option<&str>) -> Option<String> {
    if let Some(canonical) = remote.and_then(canonical_remote) {
        return Some(id_from_canonical(&canonical));
    }
    let from_remote = remote.and_then(|r| {
        let trimmed = r.trim_end_matches('/').trim_end_matches(".git");
        trimmed
            .rsplit(['/', ':'])
            .find(|s| !s.is_empty())
            .map(str::to_string)
    });
    let raw = from_remote.or_else(|| root.file_name().map(|n| n.to_string_lossy().to_string()))?;
    let id = kebab(&raw);
    (!id.is_empty()).then_some(id)
}

/// The longest id a namespace may have.
const MAX_ID_LEN: usize = 64;

/// A canonical remote as a project id: `github.com/owner/repo` is
/// `github-com-owner-repo`. One longer than a namespace may be loses its host
/// and ends in eight hex digits of a hash of the whole remote, so it stays
/// distinct and the same on every machine.
fn id_from_canonical(canonical: &str) -> String {
    let id = kebab(canonical);
    if id.len() <= MAX_ID_LEN {
        return id;
    }
    let path = canonical.split_once('/').map_or(canonical, |(_, p)| p);
    let mut base = kebab(path);
    base.truncate(MAX_ID_LEN - 9);
    let base = base.trim_end_matches('-');
    format!("{base}-{:08x}", fnv1a(canonical) >> 32)
}

/// 64-bit FNV-1a: small, and the same in every build, unlike std's hasher.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A git remote as `host/path`, lowercased, without its scheme, user,
/// password, port, trailing `/` or `.git`; `None` for a local path or a
/// `file://` URL. A copy of `canonical_remote` in the Hub's `project.rs`, which
/// keeps and looks up remotes in this form; the two must agree.
pub fn canonical_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let (authority, path) = match url.split_once("://") {
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("file") => return None,
        Some((_, rest)) => rest.split_once('/')?,
        None => {
            // scp-like `[user@]host:path`. A `/` before the `:` makes it a
            // local path, and a one-letter host a Windows drive.
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') || authority.contains('\\') {
                return None;
            }
            (authority, path)
        }
    };
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host_port.strip_prefix('[') {
        // An IPv6 literal, `[::1]:22`.
        Some(v6) => v6.split_once(']').map_or(v6, |(h, _)| h),
        None => host_port
            .rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(host_port, |(h, _)| h),
    };
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    if host.len() < 2 || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}").to_ascii_lowercase())
}

/// Lowercase ASCII alnum + dash, collapsed — matching what `Namespace::new`
/// will accept, so a repo named `My Repo!` cannot mint an invalid namespace.
fn kebab(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn kebab_keeps_only_namespace_safe_chars() {
        assert_eq!(kebab("My Repo!"), "my-repo");
        assert_eq!(kebab("CTXone"), "ctxone");
        assert_eq!(kebab("a__b--c"), "a-b-c");
        assert_eq!(kebab("--trailing--"), "trailing");
        assert_eq!(kebab(""), "");
    }

    #[test]
    fn project_id_is_the_whole_remote() {
        let root = PathBuf::from("/Users/user/checkouts/some-checkout-dir");
        // Two different paths, and ssh and https, for the same repo must agree,
        // which is what makes worktrees land in their parent's workspace.
        for remote in [
            "git@github.com:agentstatelabs/CTXone.git",
            "https://github.com/agentstatelabs/ctxone",
            "ssh://git@github.com:22/agentstatelabs/ctxone.git",
        ] {
            assert_eq!(
                project_id_for(&root, Some(remote)).unwrap(),
                "github-com-agentstatelabs-ctxone",
                "{remote}"
            );
        }
        // Repos of one name under two owners no longer share a workspace.
        assert_ne!(
            project_id_for(&root, Some("https://github.com/a/app")),
            project_id_for(&root, Some("https://github.com/b/app"))
        );
        // A remote that's a local path keeps the old rule: its last segment.
        assert_eq!(
            project_id_for(&root, Some("/srv/git/Shared-Repo.git")).unwrap(),
            "shared-repo"
        );
    }

    #[test]
    fn a_long_remote_id_fits_a_namespace_and_stays_distinct() {
        let root = PathBuf::from("/x");
        let long = |repo: &str| {
            format!(
                "https://gitlab.example.com/a-very-long-group-name/another-long-subgroup/{repo}"
            )
        };
        let a = project_id_for(&root, Some(&long("the-application"))).unwrap();
        let b = project_id_for(&root, Some(&long("the-application-two"))).unwrap();
        for id in [&a, &b] {
            assert!(id.len() <= 64, "{id} is {} bytes", id.len());
            assert!(id.starts_with("a-very-long-group-name-"), "{id}");
        }
        assert_ne!(a, b);
        // The same on every run and machine.
        assert_eq!(
            a,
            project_id_for(&root, Some(&long("the-application"))).unwrap()
        );
    }

    #[test]
    fn canonical_remote_agrees_with_the_hubs() {
        // The Hub's project.rs tests the same cases on its copy.
        for (url, want) in [
            (
                "https://user:secret@github.com:443/Owner/Repo.git/",
                Some("github.com/owner/repo"),
            ),
            (
                "git@gitlab.example.com:group/sub/app.git",
                Some("gitlab.example.com/group/sub/app"),
            ),
            (
                "ssh://git@[2001:db8::1]:2222/g/app",
                Some("2001:db8::1/g/app"),
            ),
            ("/srv/git/repo.git", None),
            ("file:///srv/git/repo.git", None),
            ("C:/repos/app", None),
            ("https://github.com", None),
        ] {
            assert_eq!(canonical_remote(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn project_id_falls_back_to_directory_name() {
        let root = PathBuf::from("/Users/user/Apps/ExampleProj");
        assert_eq!(project_id_for(&root, None).unwrap(), "exampleproj");
    }

    #[test]
    fn routed_namespace_maps_fallback_to_none() {
        assert_eq!(Routed::Existing("a".into()).namespace(), Some("a"));
        assert_eq!(Routed::Registered("b".into()).namespace(), Some("b"));
        assert_eq!(Routed::Fallback.namespace(), None);
        assert_eq!(Routed::Fallback.unresolved(), None);
        assert_eq!(Routed::Unresolved("x".into()).unresolved(), Some("x"));
    }

    /// A git repo whose detection fails is `Unresolved` — not `Fallback`
    /// (which files it in `default`) and not a fresh registration (which could
    /// split an already-registered repo into a second workspace).
    #[tokio::test]
    async fn failed_detection_is_unresolved_not_default_or_registered() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .expect("git")
        };
        git(&["init", "-q"]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://example.com/acme/widgets.git",
        ]);
        let cwd = repo.path().to_string_lossy().to_string();

        // Bind then drop, so nothing listens and the connect is refused.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let server = format!("http://127.0.0.1:{port}");
        for auto_register in [false, true] {
            let mut router = Router::new(&server, auto_register, false, None);
            let routed = router.route(Some(&cwd)).await;
            assert!(
                matches!(routed, Routed::Unresolved(ref why) if why.contains("unreachable")),
                "auto_register={auto_register}: got {routed:?}"
            );
            // Memoized, like every other outcome.
            assert_eq!(router.decisions().len(), 1);
        }
    }

    #[tokio::test]
    async fn no_cwd_uses_the_configured_fallback_namespace() {
        // An explicit `--file`/`--namespace` run has no transcript cwd to
        // route by, and must honour the caller's namespace rather than
        // silently landing in `default` — the bug that duplicated a session.
        let mut with = Router::new("http://127.0.0.1:0", false, true, Some("asd".into()));
        assert_eq!(with.route(None).await.namespace(), Some("asd"));
        assert_eq!(with.route(Some("   ")).await.namespace(), Some("asd"));

        // With no configured fallback, an unroutable transcript falls to the
        // default namespace, as `--all` sync intends.
        let mut without = Router::new("http://127.0.0.1:0", false, true, None);
        assert_eq!(without.route(None).await.namespace(), None);
    }

    #[test]
    fn dir_workspaces_routes_a_non_git_dir_to_its_basename() {
        // A non-git working directory with a distinctive name.
        let base = std::env::temp_dir().join(format!("ctxone-dw-{}", std::process::id()));
        let dir = base.join("My Scratch_App");
        std::fs::create_dir_all(&dir).expect("mkdir temp");

        // Off (default): a non-git directory has no workspace identity → routes
        // nowhere (falls to the caller's fallback / `default`).
        let off = Router::new("http://127.0.0.1:0", true, true, None);
        assert!(off.resolve_root(&dir).is_none());
        assert!(off.would_register(&dir.to_string_lossy()).is_none());

        // On: the directory itself is the root, keyed on its kebab-cased basename.
        let on = Router::new("http://127.0.0.1:0", true, true, None).with_dir_workspaces(true);
        assert_eq!(on.resolve_root(&dir).as_deref(), Some(dir.as_path()));
        assert_eq!(
            on.would_register(&dir.to_string_lossy()).as_deref(),
            Some("my-scratch-app")
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
