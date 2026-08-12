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
//! Unresolvable sessions (no cwd recorded, no git root, hub unreachable) fall
//! back to the default namespace. That is the honest answer: guessing a
//! workspace from a lossy directory label is how the old `label`-based
//! grouping produced wrong answers.

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
}

impl Routed {
    /// The namespace to write into, or `None` for the default.
    ///
    /// `WouldRegister` maps to `None`: the namespace does not exist yet, so a
    /// real write against it would 404.
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Routed::Existing(ns) | Routed::Registered(ns) => Some(ns.as_str()),
            Routed::WouldRegister(_) | Routed::Fallback => None,
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
    /// prefer their remote's `owner/repo` identity.
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
            // Short timeout: routing must never be the reason a sync stalls.
            // A miss costs one session its workspace, not the run.
            probe: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(2500))
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
        if let Some(ns) = self.detect(cwd).await {
            return Routed::Existing(ns);
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
    async fn detect(&self, cwd: &str) -> Option<String> {
        let resp = self
            .probe
            .get(format!("{}/api/projects/detect", self.server))
            .query(&[("cwd", cwd)])
            .send()
            .await
            .ok()?;
        let v: serde_json::Value = resp.json().await.ok()?;
        (v["status"] == "found")
            .then(|| v["namespace"].as_str().map(str::to_string))
            .flatten()
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

/// Derive a stable, kebab-case project id for a repo.
///
/// Prefers the remote's `owner/repo` tail so the same repo cloned to two paths
/// (or checked out as a worktree) resolves to one workspace; falls back to the
/// directory name for repos with no remote.
pub fn project_id_for(root: &std::path::Path, remote: Option<&str>) -> Option<String> {
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
    fn project_id_prefers_the_remote_tail() {
        let root = PathBuf::from("/Users/user/checkouts/some-checkout-dir");
        // Two different paths for the same repo must agree, which is what
        // makes worktrees land in their parent's workspace.
        assert_eq!(
            project_id_for(&root, Some("git@github.com:agentstatelabs/CTXone.git")).unwrap(),
            "ctxone"
        );
        assert_eq!(
            project_id_for(&root, Some("https://github.com/agentstatelabs/ctxone")).unwrap(),
            "ctxone"
        );
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
