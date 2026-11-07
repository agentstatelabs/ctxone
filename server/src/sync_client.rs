//! The Hub pushes its workspaces to a remote Hub and pulls them back
//! (ctx-hub-sync t-006).
//!
//! **Remotes** are AgentStateGraph's named remotes, kept on
//! `_control-remotes`: a name, a URL and a store (`me` names the token's
//! own store on a Hub that serves sync, [`crate::sync_service`]). A token is
//! never kept in the store. A remote names the environment variable holding
//! it, or it's in a file on this machine, `~/.ctxone/sync-tokens/<remote>`,
//! which `ctx remote add --token-stdin` writes with mode 0600.
//!
//! **What syncs where** is on [`SYNC_REF`], a local-only ref in the default
//! workspace, since it's this machine's choice:
//! - `/namespaces/<ns>`: `{remote}`, the remote the workspace syncs with.
//! - `/boundaries/<ns>`: the commits the remote's history starts at, taken
//!   when the workspace was mapped: each branch's head and the control
//!   ref's. No push goes past them, so a workspace's history from before
//!   sync stays here (HUB_SYNC.md decision 7). Raw turns were already off
//!   every branch by then (schema 3).
//! - `/last/<ns>`: when the workspace last synced, and how it went.
//!
//! **Own account only, for now.** Every workspace syncs to the same space:
//! the user's. The control ref goes with every push, and its taints and
//! reminders have no workspace, so a workspace shared with someone else's
//! space would carry all of them (AgentStateGraph Plan F t-005 scopes them).
//! Until then a mapping to another space is refused, and mapping the first
//! workspace maps the personal namespace too.
//!
//! **A sync** of a workspace pulls, then pushes:
//! - Pull: fetch every branch the remote has in the workspace, and the
//!   control ref; renumber local tasks whose ids the remote also used; then
//!   join each branch. A join merges, or for two histories with nothing in
//!   common (the first sync of a store that began on its own) merges them
//!   against an empty base (`Repository::join_fetched`).
//! - Push: every branch, local-only refs aside, through the credential scan
//!   ([`crate::push_filter`]), from the boundaries. A branch the remote moved
//!   after the fetch is rejected; the next sync pulls first.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentstategraph::sync::{PushOverride, RemoteConfig, SyncError, tracking_ref};
use agentstategraph::{CommitOptions, RepoError, Repository};
use agentstategraph_core::{IntentCategory, Namespace, ObjectId};
use agentstategraph_sync_http::HttpRemote;
use serde_json::{Value, json};

use crate::local_ref::{self, is_local_ref};
use crate::personal::PERSONAL_NAMESPACE;

/// The local-only ref holding what syncs where.
pub const SYNC_REF: &str = "_local/sync";

const KEEP: [&str; 3] = ["namespaces", "boundaries", "last"];

/// Who the sync ref's own commits are from.
const AGENT: &str = "ctxone-sync";

/// Why a sync call failed: a stable code, a message, and what to show.
#[derive(Debug)]
pub struct SyncFailure {
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
}

impl SyncFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({ "code": self.code, "error": self.message });
        if let Some(d) = &self.details {
            v["details"] = d.clone();
        }
        v
    }
}

impl From<RepoError> for SyncFailure {
    fn from(e: RepoError) -> Self {
        Self::new("store", e.to_string())
    }
}

impl From<SyncError> for SyncFailure {
    fn from(e: SyncError) -> Self {
        Self {
            code: e.code(),
            message: e.to_string(),
            details: Some(agentstategraph_sync_http::report::error(&e)),
        }
    }
}

type Result<T> = std::result::Result<T, SyncFailure>;

/// Where tokens kept on this machine are: `<home>/sync-tokens/<remote>`.
pub fn token_file(home: &Path, remote: &str) -> PathBuf {
    home.join("sync-tokens").join(remote)
}

/// `~/.ctxone`, where this machine's own files are.
pub fn default_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ctxone"))
}

fn options(intent: impl Into<String>) -> CommitOptions {
    CommitOptions::new(AGENT, IntentCategory::Custom("Sync".into()), intent)
}

fn sync_json(base: &Repository, path: &str) -> Option<Value> {
    base.get_json(SYNC_REF, path).ok()
}

fn set_sync_json(base: &Repository, path: &str, value: &Value, what: String) -> Result<()> {
    local_ref::ensure(base, SYNC_REF, &KEEP)?;
    crate::graph_write::set_json_if_changed(base, SYNC_REF, path, value, options(what))?;
    Ok(())
}

/// Which remote each workspace syncs with.
pub fn mapping(base: &Repository) -> BTreeMap<String, String> {
    let Some(Value::Object(m)) = sync_json(base, "/namespaces") else {
        return BTreeMap::new();
    };
    m.into_iter()
        .filter_map(|(ns, v)| Some((ns, v.get("remote")?.as_str()?.to_string())))
        .collect()
}

/// A workspace's repository, beside `base` in the same store.
fn workspace(base: &Repository, ns: &str) -> Result<Repository> {
    if ns == Namespace::DEFAULT {
        return Ok(base.fork_namespace(Namespace::default_ns()));
    }
    if ns == PERSONAL_NAMESPACE {
        return Ok(crate::personal::open_for_write(base)?);
    }
    let namespace =
        Namespace::new(ns).map_err(|e| SyncFailure::new("bad_namespace", e.to_string()))?;
    Ok(base.fork_namespace(namespace))
}

/// The branches sync moves: every branch but the local-only refs.
fn branches(repo: &Repository) -> Result<Vec<(String, ObjectId)>> {
    let mut all = repo.list_branches(None)?;
    all.retain(|(name, _)| !is_local_ref(name));
    Ok(all)
}

// -- Remotes ----------------------------------------------------------------

/// Add a remote, or change where it is.
pub fn add_remote(
    base: &Repository,
    name: &str,
    url: &str,
    store: Option<&str>,
    token_env: Option<&str>,
) -> Result<()> {
    let config = RemoteConfig {
        url: url.trim_end_matches('/').to_string(),
        store: store.unwrap_or(crate::sync_service::OWN_STORE).to_string(),
        token_env: token_env.map(str::to_string),
    };
    // A remote some workspaces sync with can't move to another space: they'd
    // follow it there.
    if let Some(old) = base.remote_config(name)?
        && (old.url != config.url || old.store != config.store)
        && mapping(base).values().any(|r| r == name)
    {
        return Err(SyncFailure::new(
            "remote_in_use",
            format!(
                "workspaces sync with {name}; unmap them first (ctx remote unset <namespace>) \
                 to point it elsewhere"
            ),
        ));
    }
    base.set_remote(name, &config)?;
    Ok(())
}

/// Remove a remote. Refused while a workspace syncs with it.
pub fn remove_remote(base: &Repository, name: &str) -> Result<bool> {
    let users: Vec<String> = mapping(base)
        .into_iter()
        .filter(|(_, r)| r == name)
        .map(|(ns, _)| ns)
        .collect();
    if !users.is_empty() {
        return Err(SyncFailure::new(
            "remote_in_use",
            format!(
                "{} sync with {name}; unmap them first (ctx remote unset <namespace>)",
                users.join(", ")
            ),
        ));
    }
    Ok(base.remove_remote(name)?)
}

/// Every remote, with whether it has a token here, and what syncs with it.
pub fn list(base: &Repository, home: Option<&Path>) -> Result<Value> {
    let map = mapping(base);
    let remotes: Vec<Value> = base
        .remotes()?
        .into_iter()
        .map(|(name, c)| {
            let has_token = match &c.token_env {
                Some(var) => std::env::var(var).is_ok_and(|v| !v.is_empty()),
                None => home.is_some_and(|h| token_file(h, &name).is_file()),
            };
            json!({
                "name": name,
                "url": c.url,
                "store": c.store,
                "token_env": c.token_env,
                "has_token": has_token,
                "namespaces": map.iter().filter(|(_, r)| **r == name).map(|(ns, _)| ns).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({ "remotes": remotes, "namespaces": map }))
}

fn open_remote(base: &Repository, name: &str, home: Option<&Path>) -> Result<HttpRemote> {
    let config = base
        .remote_config(name)?
        .ok_or_else(|| SyncFailure::new("no_such_remote", format!("no remote is named {name}")))?;
    let remote = HttpRemote::new(&config.url, &config.store);
    let token = match &config.token_env {
        Some(var) => match std::env::var(var) {
            Ok(t) if !t.is_empty() => Some(t),
            _ => {
                return Err(SyncFailure::new(
                    "no_token",
                    format!(
                        "{name}'s token is in ${var}, which this Hub's environment doesn't set"
                    ),
                ));
            }
        },
        None => home
            .map(|h| token_file(h, name))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
    };
    Ok(match token {
        Some(t) => remote.with_token(t),
        None => remote,
    })
}

// -- What syncs where -------------------------------------------------------

/// Sync workspace `ns` with the remote `remote`. Takes the workspace's
/// boundaries the first time: no push goes past where its branches are
/// now. Mapping the first workspace maps the personal namespace too.
/// Returns what was mapped.
pub fn map_namespace(base: &Repository, ns: &str, remote: &str) -> Result<Value> {
    let config = base.remote_config(remote)?.ok_or_else(|| {
        SyncFailure::new("no_such_remote", format!("no remote is named {remote}"))
    })?;
    let map = mapping(base);
    // Own account only: every workspace goes to the one space.
    for (other_ns, other) in &map {
        if other_ns == ns || other == remote {
            continue;
        }
        if let Some(c) = base.remote_config(other)?
            && (c.url != config.url || c.store != config.store)
        {
            return Err(SyncFailure::new(
                "one_space_only",
                format!(
                    "{other_ns} syncs with {other} ({} store {}). For now every workspace syncs \
                     to your own space: a workspace shared with another would carry all your \
                     taints and reminders with it, until AgentStateGraph scopes them by \
                     workspace",
                    c.url, c.store
                ),
            ));
        }
    }
    let mut mapped = vec![ns.to_string()];
    if ns != PERSONAL_NAMESPACE && !map.contains_key(PERSONAL_NAMESPACE) {
        mapped.push(PERSONAL_NAMESPACE.to_string());
    }
    for each in &mapped {
        let repo = workspace(base, each)?;
        // A workspace this store doesn't have yet starts empty, to take the
        // remote's.
        if repo.head("main").is_err() {
            repo.init()?;
        }
        if sync_json(base, &format!("/boundaries/{each}")).is_none() {
            let mut at: Vec<String> = branches(&repo)?
                .into_iter()
                .map(|(_, id)| id.to_hex())
                .collect();
            if let Ok(control) = base.head(agentstategraph::control::CONTROL_REF) {
                at.push(control.to_hex());
            }
            at.sort();
            at.dedup();
            set_sync_json(
                base,
                &format!("/boundaries/{each}"),
                &json!(at),
                format!("sync {each}: its history before now stays here"),
            )?;
        }
        set_sync_json(
            base,
            &format!("/namespaces/{each}"),
            &json!({ "remote": remote }),
            format!("sync {each} with {remote}"),
        )?;
    }
    Ok(json!({ "mapped": mapped, "remote": remote }))
}

/// Stop syncing workspace `ns`. Its boundaries stay, so mapping it again
/// still keeps its old history here.
pub fn unmap_namespace(base: &Repository, ns: &str) -> Result<bool> {
    if !mapping(base).contains_key(ns) {
        return Ok(false);
    }
    base.delete(
        SYNC_REF,
        &format!("/namespaces/{ns}"),
        options(format!("stop syncing {ns}")),
    )?;
    Ok(true)
}

fn boundaries(base: &Repository, ns: &str) -> Vec<ObjectId> {
    match sync_json(base, &format!("/boundaries/{ns}")) {
        Some(Value::Array(ids)) => ids
            .iter()
            .filter_map(|v| ObjectId::from_hex(v.as_str()?))
            .collect(),
        _ => Vec::new(),
    }
}

// -- Sync -------------------------------------------------------------------

/// Which halves of a sync to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Pull,
    Push,
    Both,
}

impl Mode {
    fn pulls(self) -> bool {
        matches!(self, Mode::Pull | Mode::Both)
    }
    fn pushes(self) -> bool {
        matches!(self, Mode::Push | Mode::Both)
    }
}

/// Sync every mapped workspace, or only `only`. Each reports on its own; a
/// workspace that fails doesn't stop the others.
pub fn sync(
    base: &Repository,
    home: Option<&Path>,
    only: Option<&str>,
    mode: Mode,
    overrides: &[PushOverride],
) -> Result<Value> {
    let map = mapping(base);
    if let Some(ns) = only
        && !map.contains_key(ns)
    {
        return Err(SyncFailure::new(
            "not_mapped",
            format!("{ns} doesn't sync with a remote; ctx remote set {ns} <remote>"),
        ));
    }
    let mut reports = Vec::new();
    for (ns, remote) in &map {
        if only.is_some_and(|o| o != ns) {
            continue;
        }
        let report = match sync_namespace(base, home, ns, remote, mode, overrides) {
            Ok(r) => r,
            Err(e) => json!({ "namespace": ns, "remote": remote, "error": e.to_json() }),
        };
        let ok = report.get("error").is_none()
            && report["blocked"].as_array().is_none_or(Vec::is_empty)
            && report["errors"].as_array().is_none_or(Vec::is_empty);
        let _ = set_sync_json(
            base,
            &format!("/last/{ns}"),
            &json!({ "at": chrono::Utc::now().to_rfc3339(), "ok": ok }),
            format!("{ns} synced"),
        );
        reports.push(report);
    }
    Ok(json!({ "workspaces": reports }))
}

fn sync_namespace(
    base: &Repository,
    home: Option<&Path>,
    ns: &str,
    remote_name: &str,
    mode: Mode,
    overrides: &[PushOverride],
) -> Result<Value> {
    let remote = open_remote(base, remote_name, home)?;
    let repo = Arc::new(workspace(base, ns)?);
    let mut report = json!({
        "namespace": ns,
        "remote": remote_name,
        "pulled": [],
        "pushed": [],
        "blocked": [],
        "errors": [],
    });
    if mode.pulls() {
        let fetched = repo.fetch(remote_name, &remote, &[])?;
        report["fetch"] = agentstategraph_sync_http::report::fetch(&fetched);
        for (branch, _) in repo.remote_refs(remote_name)? {
            if is_local_ref(&branch) {
                continue;
            }
            match pull_branch(&repo, remote_name, &branch) {
                Ok(v) => push_to(&mut report, "pulled", v),
                Err(e) => push_to(
                    &mut report,
                    "errors",
                    json!({ "branch": branch, "pull": e.to_json() }),
                ),
            }
        }
    }
    if mode.pushes() {
        let shallow = boundaries(base, ns);
        for (branch, _) in branches(&repo)? {
            let opts = crate::push_filter::push_options(shallow.clone(), overrides.to_vec());
            match repo.push_with(remote_name, &remote, &branch, &opts) {
                Ok(pushed) if pushed.updated.is_empty() => {}
                Ok(pushed) => {
                    let mut v = agentstategraph_sync_http::report::push(&pushed);
                    v["branch"] = json!(branch);
                    push_to(&mut report, "pushed", v);
                }
                Err(SyncError::Blocked { findings, .. }) => push_to(
                    &mut report,
                    "blocked",
                    json!({
                        "branch": branch,
                        "findings": findings.iter().map(crate::push_filter::finding_json).collect::<Vec<_>>(),
                    }),
                ),
                Err(e) => push_to(
                    &mut report,
                    "errors",
                    json!({ "branch": branch, "push": SyncFailure::from(e).to_json() }),
                ),
            }
        }
    }
    Ok(report)
}

fn push_to(report: &mut Value, key: &str, item: Value) {
    if let Some(list) = report[key].as_array_mut() {
        list.push(item);
    }
}

/// Renumber colliding local tasks, then join the fetched branch.
fn pull_branch(repo: &Arc<Repository>, remote_name: &str, branch: &str) -> Result<Value> {
    let opts = CommitOptions::new(
        AGENT,
        IntentCategory::Merge,
        format!("pull {branch} from {remote_name}"),
    );
    let mut renamed = json!({});
    let mut plan_conflicts = json!([]);
    if repo.head(branch).is_ok() {
        let tasks = agentstategraph_tasks::TaskStore::new(Arc::clone(repo), "/plans", AGENT);
        let r = tasks
            .renumber_before_merge(branch, &tracking_ref(remote_name, branch))
            .map_err(|e| SyncFailure::new("renumber_failed", e.to_string()))?;
        renamed = r
            .renamed
            .iter()
            .map(|(plan, moves)| {
                let moves: Vec<Value> = moves
                    .iter()
                    .map(|(a, b)| json!([a.to_string(), b.to_string()]))
                    .collect();
                (plan.clone(), Value::Array(moves))
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        plan_conflicts = json!(r.plan_conflicts);
    }
    let outcome = repo.join_fetched(remote_name, branch, opts)?;
    let mut v = agentstategraph_sync_http::report::pull_outcome(&outcome);
    v["branch"] = json!(branch);
    v["renumbered"] = renamed;
    v["plan_conflicts"] = plan_conflicts;
    Ok(v)
}

// -- Status -----------------------------------------------------------------

/// Where each mapped workspace's branches stand against the remote, as the
/// last fetch or push left it. It doesn't ask the remote.
pub fn status(base: &Repository) -> Result<Value> {
    let mut out = Vec::new();
    for (ns, remote) in mapping(base) {
        let repo = workspace(base, &ns)?;
        let theirs: BTreeMap<String, ObjectId> = repo.remote_refs(&remote)?.into_iter().collect();
        let mut rows = Vec::new();
        let mut names: Vec<String> = branches(&repo)?.into_iter().map(|(b, _)| b).collect();
        for b in theirs.keys() {
            if !names.contains(b) {
                names.push(b.clone());
            }
        }
        names.sort();
        for b in names {
            let ours = repo.head(&b).ok();
            let at = theirs.get(&b).copied();
            let state = match (ours, at) {
                (Some(o), Some(t)) if o == t => "up_to_date",
                (Some(_), None) => "not_pushed",
                (None, Some(_)) => "not_pulled",
                (None, None) => continue,
                (Some(o), Some(t)) => match repo.merge_base(&tracking_ref(&remote, &b), &b) {
                    Ok(base_id) if base_id == t => "ahead",
                    Ok(base_id) if base_id == o => "behind",
                    Ok(_) => "diverged",
                    Err(_) => "unrelated",
                },
            };
            rows.push(json!({
                "branch": b,
                "state": state,
                "local": ours.map(|o| o.short()),
                "remote": at.map(|t| t.short()),
            }));
        }
        out.push(json!({
            "namespace": ns,
            "remote": remote,
            "branches": rows,
            "last": sync_json(base, &format!("/last/{ns}")),
        }));
    }
    Ok(json!({ "workspaces": out }))
}

/// A fingerprint of every mapped workspace's branch heads, for the
/// background sync to tell whether anything changed since it last ran.
pub fn fingerprint(base: &Repository) -> String {
    let mut parts = Vec::new();
    for ns in mapping(base).keys() {
        if let Ok(repo) = workspace(base, ns)
            && let Ok(bs) = branches(&repo)
        {
            for (b, id) in bs {
                parts.push(format!("{ns}/{b}={}", id.short()));
            }
        }
    }
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph_storage::MemoryStorage;

    fn store() -> Repository {
        let repo = Repository::new(Box::new(MemoryStorage::new()));
        repo.init().unwrap();
        repo
    }

    #[test]
    fn mapping_takes_boundaries_and_the_personal_namespace() {
        let repo = store();
        repo.set_json(
            "main",
            "/memory/f",
            &json!("x"),
            CommitOptions::new("t", IntentCategory::Checkpoint, "w"),
        )
        .unwrap();
        add_remote(&repo, "hub", "https://hub.example", None, None).unwrap();
        let mapped = map_namespace(&repo, "default", "hub").unwrap();
        assert_eq!(mapped["mapped"], json!(["default", "_personal"]));
        assert!(repo.is_local_only(SYNC_REF).unwrap());
        let at = boundaries(&repo, "default");
        assert!(at.contains(&repo.head("main").unwrap()));
        assert_eq!(mapping(&repo).get("_personal"), Some(&"hub".to_string()));

        // Another space is refused; the same space under another name isn't.
        add_remote(&repo, "team", "https://team.example", None, None).unwrap();
        repo.create_namespace("proj").unwrap();
        let err = map_namespace(&repo, "proj", "team").unwrap_err();
        assert_eq!(err.code, "one_space_only");
        add_remote(&repo, "again", "https://hub.example", None, None).unwrap();
        map_namespace(&repo, "proj", "again").unwrap();

        // A remote in use can't move or go.
        assert_eq!(
            add_remote(&repo, "hub", "https://elsewhere.example", None, None)
                .unwrap_err()
                .code,
            "remote_in_use"
        );
        assert_eq!(
            remove_remote(&repo, "hub").unwrap_err().code,
            "remote_in_use"
        );
        assert!(unmap_namespace(&repo, "proj").unwrap());
        assert!(!unmap_namespace(&repo, "proj").unwrap());
    }

    #[test]
    fn a_token_file_is_read_and_never_kept_in_the_store() {
        let repo = store();
        let home = tempfile::tempdir().unwrap();
        add_remote(&repo, "hub", "https://hub.example", None, None).unwrap();
        std::fs::create_dir_all(home.path().join("sync-tokens")).unwrap();
        std::fs::write(token_file(home.path(), "hub"), "tok-123\n").unwrap();
        let listed = list(&repo, Some(home.path())).unwrap();
        assert_eq!(listed["remotes"][0]["has_token"], true);
        assert!(!listed.to_string().contains("tok-123"));
        assert!(open_remote(&repo, "hub", Some(home.path())).is_ok());
        let paths = repo.list_paths("main", "/", None).unwrap();
        assert!(paths.iter().all(|p| !p.contains("tok")));
    }
}
