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
    /// cwd → routing outcome. Includes negative results so a directory that
    /// cannot be resolved is not re-probed once per transcript.
    memo: HashMap<String, Routed>,
    /// namespace → client. `None` key is the default-namespace client.
    clients: HashMap<Option<String>, reqwest::Client>,
}

impl Router {
    pub fn new(server: &str, auto_register: bool, dry_run: bool) -> Self {
        Self {
            server: server.to_string(),
            dry_run,
            // Short timeout: routing must never be the reason a sync stalls.
            // A miss costs one session its workspace, not the run.
            probe: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(2500))
                .build()
                .unwrap_or_default(),
            auto_register,
            memo: HashMap::new(),
            clients: HashMap::new(),
        }
    }

    /// Route one transcript. `cwd` is whatever the source recorded.
    pub async fn route(&mut self, cwd: Option<&str>) -> Routed {
        let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
            return Routed::Fallback;
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
            return Routed::Fallback;
        }
        if self.dry_run {
            // Work out the id without calling the registry, so a dry run can
            // show the intended workspace while staying read-only.
            return match self.would_register(cwd) {
                Some(id) => Routed::WouldRegister(id),
                None => Routed::Fallback,
            };
        }
        match self.register(cwd).await {
            Some(ns) => Routed::Registered(ns),
            None => Routed::Fallback,
        }
    }

    /// The id `register` would mint, computed locally (git only, no HTTP).
    fn would_register(&self, cwd: &str) -> Option<String> {
        let dir = std::path::Path::new(cwd);
        if !dir.is_dir() {
            return None;
        }
        let root = crate::find_git_root(dir)?;
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
        let root = crate::find_git_root(dir)?;
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
        self.clients
            .entry(key)
            .or_insert_with(|| build(ns))
            .clone()
    }

    /// cwd → namespace decisions made this run, for the summary line.
    pub fn decisions(&self) -> Vec<(&str, &Routed)> {
        let mut out: Vec<(&str, &Routed)> =
            self.memo.iter().map(|(k, v)| (k.as_str(), v)).collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }
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
    let raw = from_remote.or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
    })?;
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
}
