//! Raw session turns, on a local-only ref of their own (ctx-hub-sync t-002).
//!
//! A session has two parts:
//! - **Its envelope:** title, meta, provenance, burn, usage, analysis,
//!   summary and decisions, at `/sessions/<sid>` on a workspace's branches.
//!   The envelope syncs.
//! - **Its raw turns:** the transcript itself, at `/turns/<sid>/t<NNNN>` on
//!   [`TURNS_REF`] in the same workspace.
//!
//! Turns stay on this machine. They're most of a store's size, only the
//! machine that ran a session can scrape them again, and they're where
//! credentials turn up. [`TURNS_REF`] is marked local-only
//! (`Repository::set_local_only`), so AgentStateGraph never pushes it and a
//! Hub never offers it to a fetch.
//!
//! A workspace has one turns ref, whatever branch a session ran on: session
//! ids are unique, so the branch adds nothing.
//!
//! Before schema 3, turns sat in the envelope at `/sessions/<sid>/turns`, on
//! whatever branch the ingest named. [`migrate_store`] moves them, once.

use std::collections::{BTreeMap, HashMap, HashSet};

use agentstategraph::{CommitOptions, RepoError, Repository};
use agentstategraph_core::{IntentCategory, Namespace, Node, Object, ObjectId};
use tracing::{info, warn};

use crate::graph_write::set_json_if_changed;
use crate::local_ref::is_local_ref;

/// The ref holding a workspace's raw session turns
/// ([`crate::local_ref`]).
pub const TURNS_REF: &str = "_local/turns";

/// Where the turns sit on [`TURNS_REF`]: `/turns/<sid>/t<NNNN>`.
const TURNS_ROOT: &str = "/turns";

/// Scratch space on [`TURNS_REF`] that [`migrate_namespace`] stores the
/// stripped envelopes in before it relinks them onto a branch.
const STRIP_ROOT: &str = "/_strip";

/// The top-level keys the turns ref holds besides `/_meta`.
const KEEP: [&str; 2] = ["turns", "_strip"];

/// Who the turns ref's own commits are from.
const AGENT: &str = "ctxone-turns";

/// A session's turns: `/turns/<sid>` on [`TURNS_REF`].
pub fn session_path(sid: &str) -> String {
    format!("{TURNS_ROOT}/{sid}")
}

/// One turn: `/turns/<sid>/t<NNNN>`. The `t` makes the segment a map key, not
/// a list index, which can't be created under an empty parent.
pub fn turn_path(sid: &str, idx: u32) -> String {
    format!("{TURNS_ROOT}/{sid}/t{idx:04}")
}

fn options(intent: impl Into<String>) -> CommitOptions {
    CommitOptions::new(AGENT, IntentCategory::Migrate, intent)
}

/// Create the workspace's turns ref if it's missing: branched from `main`,
/// marked local-only, then cut down so it holds nothing but turns. The
/// workspace's `main` must exist.
pub fn ensure(repo: &Repository) -> Result<(), RepoError> {
    crate::local_ref::ensure(repo, TURNS_REF, &KEEP)
}

/// Every turn a session has, as `{ "tNNNN": turn }`. `None` when it has none
/// here.
pub fn read(repo: &Repository, sid: &str) -> Option<serde_json::Value> {
    repo.get_json(TURNS_REF, &session_path(sid)).ok()
}

/// One turn.
pub fn read_turn(repo: &Repository, sid: &str, idx: u32) -> Result<serde_json::Value, RepoError> {
    repo.get_json(TURNS_REF, &turn_path(sid, idx))
}

/// The keys (`tNNNN`) of a session's turns, sorted. Reads one node, not the
/// turns.
pub fn keys(repo: &Repository, sid: &str) -> Vec<String> {
    match repo.get(TURNS_REF, &session_path(sid)) {
        Ok(Object::Node(Node::Map(turns))) => turns.into_keys().collect(),
        _ => Vec::new(),
    }
}

/// The sessions that have turns in this workspace. Reads one node.
pub fn sessions(repo: &Repository) -> HashSet<String> {
    match repo.get(TURNS_REF, TURNS_ROOT) {
        Ok(Object::Node(Node::Map(sessions))) => sessions.into_keys().collect(),
        _ => HashSet::new(),
    }
}

/// Write all of a session's turns, replacing what it had. A write that
/// changes nothing makes no commit.
pub fn write_all(
    repo: &Repository,
    sid: &str,
    turns: &serde_json::Value,
    opts: CommitOptions,
) -> Result<ObjectId, RepoError> {
    ensure(repo)?;
    set_json_if_changed(repo, TURNS_REF, &session_path(sid), turns, opts)
}

/// Write one turn. A write that changes nothing makes no commit.
pub fn write_turn(
    repo: &Repository,
    sid: &str,
    idx: u32,
    turn: &serde_json::Value,
    opts: CommitOptions,
) -> Result<ObjectId, RepoError> {
    ensure(repo)?;
    set_json_if_changed(repo, TURNS_REF, &turn_path(sid, idx), turn, opts)
}

/// Delete a session's turns. Returns how many leaves went: 0 when it had
/// none, and then nothing is committed.
pub fn delete(repo: &Repository, sid: &str, opts: CommitOptions) -> Result<usize, RepoError> {
    let path = session_path(sid);
    let leaves = repo
        .list_paths(TURNS_REF, &path, None)
        .map(|p| p.len())
        .unwrap_or(0);
    if leaves == 0 && repo.get(TURNS_REF, &path).is_err() {
        return Ok(0);
    }
    repo.delete(TURNS_REF, &path, opts)?;
    Ok(leaves)
}

/// What [`move_session`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Moved {
    /// The source had turns for the session.
    pub found: bool,
    /// They were written to the target.
    pub wrote_target: bool,
    /// They were removed from the source.
    pub deleted_source: bool,
}

/// Move a session's turns to another workspace of the same store. Both
/// share one object store, so the turns are relinked, not copied: the target
/// gets the very subtree the source had. With `keep_target`, turns the
/// target already has stay as they are. The source's are removed only once
/// the target holds the session's turns.
pub fn move_session(
    src: &Repository,
    dst: &Repository,
    sid: &str,
    keep_target: bool,
    agent: &str,
) -> Result<Moved, RepoError> {
    let path = session_path(sid);
    let Ok(turns) = src.get(TURNS_REF, &path) else {
        return Ok(Moved::default());
    };
    let mut moved = Moved {
        found: true,
        ..Moved::default()
    };
    ensure(dst)?;
    let held = dst.get(TURNS_REF, &path).ok();
    let write = match &held {
        Some(current) => !keep_target && current.id() != turns.id(),
        None => true,
    };
    if write {
        let opts = CommitOptions::new(
            agent,
            IntentCategory::Custom("Observe".to_string()),
            format!("move session {sid}'s turns from {}", src.namespace()),
        )
        .with_tags(vec![
            format!("session:{sid}"),
            "kind:session-move".to_string(),
        ]);
        dst.set(TURNS_REF, &path, &turns, opts)?;
        moved.wrote_target = true;
    }
    if dst.get(TURNS_REF, &path).is_ok() {
        let opts = CommitOptions::new(
            agent,
            IntentCategory::Rollback,
            format!("move session {sid}'s turns to {}", dst.namespace()),
        )
        .with_tags(vec![
            format!("session:{sid}"),
            "kind:session-move".to_string(),
        ]);
        src.delete(TURNS_REF, &path, opts)?;
        moved.deleted_source = true;
    }
    Ok(moved)
}

// -- Migration (CTXone schema 3) ---------------------------------------------

/// What [`migrate_store`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Workspaces that had turns to move.
    pub namespaces: usize,
    /// Branches whose envelopes lost their turns.
    pub branches: usize,
    /// Sessions whose turns are on a turns ref now.
    pub sessions: usize,
}

/// Move every workspace's turns onto its turns ref, and off its branches.
/// Idempotent and safe to stop at any point: the turns are linked onto the
/// turns ref before any branch drops them, and a run finds what's left to
/// do from the store itself.
pub fn migrate_store(repo: &Repository) -> Result<MigrationReport, RepoError> {
    let started = std::time::Instant::now();
    let mut names = repo.list_namespaces()?;
    if !names.iter().any(|n| n.as_str() == Namespace::DEFAULT) {
        names.insert(0, Namespace::default_ns());
    }
    let mut report = MigrationReport::default();
    for ns in names {
        let forked;
        let workspace = if ns.as_str() == repo.namespace().as_str() {
            repo
        } else {
            forked = repo.fork_namespace(ns.clone());
            &forked
        };
        let (branches, sessions) = migrate_namespace(workspace)?;
        if sessions > 0 || branches > 0 {
            report.namespaces += 1;
            report.branches += branches;
            report.sessions += sessions;
            info!(
                namespace = %ns,
                branches,
                sessions,
                "moved raw session turns onto {TURNS_REF}"
            );
        }
    }
    info!(
        namespaces = report.namespaces,
        branches = report.branches,
        sessions = report.sessions,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "turns migration finished"
    );
    Ok(report)
}

/// A session node holding turns, as one branch has it.
struct WithTurns {
    /// The node's other children: its envelope.
    envelope: BTreeMap<String, ObjectId>,
    turns: ObjectId,
}

/// One workspace: returns (branches stripped, sessions moved).
fn migrate_namespace(repo: &Repository) -> Result<(usize, usize), RepoError> {
    // Every branch's /sessions, and the session nodes that hold turns.
    let mut branches: Vec<(String, BTreeMap<String, ObjectId>)> = Vec::new();
    let mut nodes: HashMap<ObjectId, WithTurns> = HashMap::new();
    // Which branch and session to read each node at.
    let mut node_at: HashMap<ObjectId, (String, String)> = HashMap::new();
    let mut listed = repo.list_branches(None)?;
    listed.retain(|(name, _)| !is_local_ref(name));
    // main first, so it's the copy a tie keeps.
    listed.sort_by_key(|(name, _)| name != "main");
    for (branch, _) in listed {
        let Ok(Object::Node(Node::Map(sessions))) = repo.get(&branch, "/sessions") else {
            continue;
        };
        let mut any = false;
        for (sid, node_id) in &sessions {
            if nodes.contains_key(node_id) {
                any = true;
                continue;
            }
            let Ok(Object::Node(Node::Map(children))) =
                repo.get(&branch, &format!("/sessions/{sid}"))
            else {
                continue;
            };
            let Some(turns) = children.get("turns").copied() else {
                continue;
            };
            let mut envelope = children;
            envelope.remove("turns");
            nodes.insert(*node_id, WithTurns { envelope, turns });
            node_at.insert(*node_id, (branch.clone(), sid.clone()));
            any = true;
        }
        if any {
            branches.push((branch, sessions));
        }
    }
    if branches.is_empty() {
        return Ok((0, 0));
    }
    ensure(repo)?;

    // 1. Link the turns onto the turns ref, before any branch drops them.
    //    A session with different turns on two branches keeps the longer
    //    transcript: a transcript only grows.
    let mut turns: BTreeMap<String, ObjectId> = match repo.get(TURNS_REF, TURNS_ROOT) {
        Ok(Object::Node(Node::Map(held))) => held,
        _ => BTreeMap::new(),
    };
    let mut lengths: HashMap<ObjectId, usize> = HashMap::new();
    let mut length = |id: &ObjectId, branch: &str, path: &str| -> usize {
        *lengths
            .entry(*id)
            .or_insert_with(|| match repo.get(branch, path) {
                Ok(Object::Node(Node::Map(m))) => m.len(),
                Ok(Object::Node(Node::List(l))) | Ok(Object::Node(Node::Set(l))) => l.len(),
                Ok(_) => 1,
                Err(_) => 0,
            })
    };
    for (branch, sessions) in &branches {
        for (sid, node_id) in sessions {
            let Some(node) = nodes.get(node_id) else {
                continue;
            };
            let path = format!("/sessions/{sid}/turns");
            let replace = match turns.get(sid) {
                None => true,
                Some(held) if *held == node.turns => false,
                Some(held) => {
                    let held = *held;
                    let held_len = length(&held, TURNS_REF, &session_path(sid));
                    length(&node.turns, branch, &path) > held_len
                }
            };
            if replace {
                turns.insert(sid.clone(), node.turns);
            }
        }
    }
    let wanted = Object::Node(Node::Map(turns.clone()));
    if repo.get(TURNS_REF, TURNS_ROOT).map(|o| o.id()).ok() != Some(wanted.id()) {
        repo.set(
            TURNS_REF,
            TURNS_ROOT,
            &wanted,
            options(format!("move {} sessions' raw turns here", turns.len())),
        )?;
    }

    // 2. Store each envelope without its turns. They're written as JSON in
    //    one commit on the turns ref, which stores each node; nothing else
    //    can store an object without committing it where it's wanted.
    let mut envelopes = serde_json::Map::new();
    for (node_id, node) in &nodes {
        if node.envelope.is_empty() {
            continue;
        }
        let (branch, sid) = &node_at[node_id];
        let paths: Vec<String> = node
            .envelope
            .keys()
            .map(|key| format!("/sessions/{sid}/{key}"))
            .collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let values = repo.get_json_many(branch, &refs)?;
        let mut envelope = serde_json::Map::new();
        for (key, value) in node.envelope.keys().zip(values) {
            match value {
                Some(value) => {
                    envelope.insert(key.clone(), value);
                }
                None => {
                    return Err(RepoError::InvalidOperation(format!(
                        "couldn't read /sessions/{sid}/{key} on {branch} to move its turns"
                    )));
                }
            }
        }
        envelopes.insert(node_id.to_hex(), serde_json::Value::Object(envelope));
    }
    let mut stripped: HashMap<ObjectId, ObjectId> = HashMap::new();
    if !envelopes.is_empty() {
        repo.set_json(
            TURNS_REF,
            STRIP_ROOT,
            &serde_json::Value::Object(envelopes),
            options("store session envelopes without their turns"),
        )?;
        if let Ok(Object::Node(Node::Map(stored))) = repo.get(TURNS_REF, STRIP_ROOT) {
            for (hex, id) in stored {
                if let Some(orig) = ObjectId::from_hex(&hex) {
                    stripped.insert(orig, id);
                }
            }
        }
    }

    // 3. Drop the turns from each branch, in one commit per branch. A session
    //    that held nothing but turns goes from the branch: the session list
    //    never showed one.
    let mut changed = 0;
    for (branch, sessions) in &branches {
        let mut after: BTreeMap<String, ObjectId> = BTreeMap::new();
        for (sid, node_id) in sessions {
            match nodes.get(node_id) {
                None => {
                    after.insert(sid.clone(), *node_id);
                }
                Some(node) if node.envelope.is_empty() => {}
                Some(_) => {
                    let id = stripped.get(node_id).copied().ok_or_else(|| {
                        RepoError::InvalidOperation(format!(
                            "session {sid}'s envelope wasn't stored, so {branch} keeps its turns"
                        ))
                    })?;
                    after.insert(sid.clone(), id);
                }
            }
        }
        // Read again just before writing: the head this was worked out from
        // must still be the head.
        let Ok(Object::Node(Node::Map(now))) = repo.get(branch, "/sessions") else {
            continue;
        };
        if now != *sessions {
            warn!(
                branch = %branch,
                namespace = %repo.namespace(),
                "sessions changed during the turns migration; it runs again at the next start"
            );
            continue;
        }
        repo.set(
            branch,
            "/sessions",
            &Object::Node(Node::Map(after)),
            options(format!("move raw session turns to {TURNS_REF}")),
        )?;
        changed += 1;
    }
    if repo.get(TURNS_REF, STRIP_ROOT).is_ok() {
        repo.delete(
            TURNS_REF,
            STRIP_ROOT,
            options("drop the stripped envelopes"),
        )?;
    }
    Ok((changed, turns.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph::sync::InProcessRemote;
    use agentstategraph_storage::MemoryStorage;
    use serde_json::json;

    fn opts() -> CommitOptions {
        CommitOptions::new("test", IntentCategory::Checkpoint, "seed")
    }

    fn store() -> Repository {
        let repo = Repository::new(Box::new(MemoryStorage::new()));
        repo.init().unwrap();
        repo
    }

    fn put(repo: &Repository, branch: &str, path: &str, value: serde_json::Value) {
        repo.set_json(branch, path, &value, opts()).unwrap();
    }

    fn turns(n: usize, word: &str) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for i in 0..n {
            m.insert(format!("t{i:04}"), json!({ "text": format!("{word} {i}") }));
        }
        serde_json::Value::Object(m)
    }

    /// How sessions were stored before schema 3: turns in the envelope, on
    /// main and on a branch, in two workspaces.
    fn legacy() -> Repository {
        let repo = store();
        put(&repo, "main", "/memory/facts/f1", json!("a fact"));
        put(
            &repo,
            "main",
            "/sessions/s1",
            json!({ "title": "one", "meta": { "source": "claude" }, "turns": turns(3, "one") }),
        );
        put(&repo, "main", "/sessions/s2", json!({ "title": "two" }));
        put(
            &repo,
            "main",
            "/sessions/s3",
            json!({ "turns": turns(1, "three") }),
        );
        repo.branch("feat", "main").unwrap();
        put(
            &repo,
            "feat",
            "/sessions/s4",
            json!({ "title": "four", "turns": turns(2, "four") }),
        );
        // s1 grew on feat after the branch was made.
        put(&repo, "feat", "/sessions/s1/turns", turns(5, "one"));
        let proj = repo.fork_namespace(Namespace::new("proj").unwrap());
        proj.init().unwrap();
        put(
            &proj,
            "main",
            "/sessions/s5",
            json!({ "title": "five", "turns": turns(2, "five") }),
        );
        repo
    }

    fn proj(repo: &Repository) -> Repository {
        repo.fork_namespace(Namespace::new("proj").unwrap())
    }

    #[test]
    fn migration_moves_turns_off_every_branch() {
        let repo = legacy();
        let report = migrate_store(&repo).unwrap();
        assert_eq!(report.namespaces, 2);
        assert_eq!(report.branches, 3);
        // s1, s3 and s4 in the default workspace, s5 in proj.
        assert_eq!(report.sessions, 4);

        for branch in ["main", "feat"] {
            let paths = repo.list_paths(branch, "/sessions", None).unwrap();
            assert!(
                paths.iter().all(|p| !p.contains("/turns")),
                "{branch}: {paths:?}"
            );
        }
        // Envelopes stay, as they were.
        assert_eq!(repo.get_json("main", "/sessions/s1/title").unwrap(), "one");
        assert_eq!(
            repo.get_json("main", "/sessions/s1/meta").unwrap(),
            json!({ "source": "claude" })
        );
        assert_eq!(repo.get_json("feat", "/sessions/s4/title").unwrap(), "four");
        assert_eq!(
            repo.get_json("main", "/sessions/s2").unwrap(),
            json!({ "title": "two" })
        );
        // A session that was only turns leaves the branch.
        assert!(repo.get("main", "/sessions/s3").is_err());
        assert_eq!(repo.get_json("main", "/memory/facts/f1").unwrap(), "a fact");

        // The turns are on the turns ref, s1's longer copy from feat.
        assert_eq!(read(&repo, "s1"), Some(turns(5, "one")));
        assert_eq!(read(&repo, "s3"), Some(turns(1, "three")));
        assert_eq!(read(&repo, "s4"), Some(turns(2, "four")));
        assert_eq!(read(&proj(&repo), "s5"), Some(turns(2, "five")));
        assert_eq!(
            sessions(&repo),
            HashSet::from(["s1".to_string(), "s3".to_string(), "s4".to_string()])
        );
        assert!(repo.is_local_only(TURNS_REF).unwrap());
        // It holds turns and nothing else.
        let Object::Node(Node::Map(root)) = repo.get(TURNS_REF, "/").unwrap() else {
            panic!("root isn't a map");
        };
        assert_eq!(
            root.keys().collect::<Vec<_>>(),
            vec!["_meta", "turns"],
            "the turns ref holds only turns"
        );
    }

    #[test]
    fn migration_runs_again_without_changing_anything() {
        let repo = legacy();
        migrate_store(&repo).unwrap();
        let heads = |r: &Repository| {
            r.list_branches(None)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>()
        };
        let before = (heads(&repo), heads(&proj(&repo)));
        let again = migrate_store(&repo).unwrap();
        assert_eq!(again, MigrationReport::default());
        assert_eq!((heads(&repo), heads(&proj(&repo))), before);
    }

    #[test]
    fn migration_finishes_what_a_stopped_run_left() {
        let repo = legacy();
        // A run that stopped after linking main's copy of s1's turns, before
        // any branch lost them, and left its scratch space behind.
        ensure(&repo).unwrap();
        let s1 = repo.get("main", "/sessions/s1/turns").unwrap();
        repo.set(TURNS_REF, &session_path("s1"), &s1, options("partial"))
            .unwrap();
        put(
            &repo,
            TURNS_REF,
            "/_strip/stale",
            json!({ "title": "stale" }),
        );
        migrate_store(&repo).unwrap();
        // feat's longer copy wins over the one already linked.
        assert_eq!(read(&repo, "s1"), Some(turns(5, "one")));
        assert_eq!(read(&repo, "s4"), Some(turns(2, "four")));
        assert!(repo.get("main", "/sessions/s1/turns").is_err());
        assert!(repo.get("feat", "/sessions/s1/turns").is_err());
        assert!(repo.get(TURNS_REF, STRIP_ROOT).is_err());
    }

    #[test]
    fn a_store_without_turns_gets_no_turns_ref() {
        let repo = store();
        put(&repo, "main", "/sessions/s2", json!({ "title": "two" }));
        let head = repo.head("main").unwrap();
        assert_eq!(migrate_store(&repo).unwrap(), MigrationReport::default());
        assert!(repo.head(TURNS_REF).is_err());
        assert_eq!(repo.head("main").unwrap(), head);
    }

    #[test]
    fn writes_create_the_ref_local_only_and_holding_only_turns() {
        let repo = store();
        put(&repo, "main", "/memory/facts/f1", json!("a fact"));
        write_turn(&repo, "s1", 0, &json!({ "text": "hi" }), opts()).unwrap();
        write_turn(&repo, "s1", 1, &json!({ "text": "there" }), opts()).unwrap();
        assert!(repo.is_local_only(TURNS_REF).unwrap());
        assert_eq!(keys(&repo, "s1"), vec!["t0000", "t0001"]);
        assert_eq!(
            read_turn(&repo, "s1", 1).unwrap(),
            json!({ "text": "there" })
        );
        assert!(repo.get(TURNS_REF, "/memory").is_err());
        // An unchanged write commits nothing.
        let head = repo.head(TURNS_REF).unwrap();
        write_all(&repo, "s1", &read(&repo, "s1").unwrap(), opts()).unwrap();
        assert_eq!(repo.head(TURNS_REF).unwrap(), head);
        // Nothing reached main.
        assert!(repo.get("main", "/sessions").is_err());
    }

    #[test]
    fn the_turns_ref_is_never_pushed_or_offered() {
        let repo = store();
        put(&repo, "main", "/sessions/s1/title", json!("one"));
        write_all(&repo, "s1", &turns(2, "transcript"), opts()).unwrap();
        let hub = store();
        let remote = InProcessRemote::new(&hub);
        // The hub's main is only `init`'s empty first commit, which a push
        // may replace once a fetch has seen it.
        repo.fetch("hub", &remote, &[]).unwrap();
        let err = repo.push("hub", &remote, TURNS_REF).unwrap_err();
        assert_eq!(err.code(), "local_only_ref");
        assert!(
            repo.sync_refs()
                .unwrap()
                .iter()
                .all(|r| r.name != TURNS_REF),
            "a store doesn't offer its turns ref"
        );
        // A push of main sends the envelope and no turns.
        repo.push("hub", &remote, "main").unwrap();
        assert_eq!(hub.get_json("main", "/sessions/s1/title").unwrap(), "one");
        assert!(hub.get(TURNS_REF, "/").is_err());
        assert!(hub.get("main", "/turns").is_err());
    }

    #[test]
    fn delete_and_move_carry_the_turns() {
        let repo = legacy();
        migrate_store(&repo).unwrap();
        let other = proj(&repo);
        let moved = move_session(&repo, &other, "s1", false, "test").unwrap();
        assert_eq!(
            moved,
            Moved {
                found: true,
                wrote_target: true,
                deleted_source: true
            }
        );
        assert_eq!(read(&other, "s1"), Some(turns(5, "one")));
        assert_eq!(read(&repo, "s1"), None);
        assert_eq!(
            move_session(&repo, &other, "s1", false, "test").unwrap(),
            Moved::default()
        );

        assert_eq!(delete(&other, "s1", opts()).unwrap(), 5);
        assert_eq!(read(&other, "s1"), None);
        assert_eq!(delete(&other, "s1", opts()).unwrap(), 0);
    }

    /// Times the migration on a large store, in a SQLite file. Run by hand:
    /// `cargo test -p ctxone-hub --release --lib turns::tests::migration_timing -- --ignored --nocapture`.
    /// `CTX_BENCH_SESSIONS`, `CTX_BENCH_TURNS` and `CTX_BENCH_BRANCHES` size it.
    #[test]
    #[ignore]
    fn migration_timing_on_a_large_store() {
        let n = |name: &str, default: usize| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let (sessions, per_session, extra_branches) = (
            n("CTX_BENCH_SESSIONS", 800),
            n("CTX_BENCH_TURNS", 60),
            n("CTX_BENCH_BRANCHES", 10),
        );
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("bench.db");
        let storage = agentstategraph_storage::SqliteStorage::open(db.to_str().unwrap()).unwrap();
        let repo = Repository::new(Box::new(storage));
        repo.init().unwrap();
        let seed = std::time::Instant::now();
        let turn_text = "x".repeat(2000);
        for s in 0..sessions {
            let mut turns = serde_json::Map::new();
            for t in 0..per_session {
                turns.insert(
                    format!("t{t:04}"),
                    json!({ "text": format!("{s}/{t} {turn_text}"), "model": "m" }),
                );
            }
            put(
                &repo,
                "main",
                &format!("/sessions/s{s:05}"),
                json!({
                    "title": format!("session {s}"),
                    "meta": { "source": "claude", "updated_at": "2026-11-06T00:00:00Z" },
                    "turns": turns,
                }),
            );
        }
        for b in 0..extra_branches {
            repo.branch(&format!("feature/{b}"), "main").unwrap();
        }
        let seeded = seed.elapsed();
        let bytes = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);

        let started = std::time::Instant::now();
        let report = migrate_store(&repo).unwrap();
        let took = started.elapsed();
        let after = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
        eprintln!(
            "{sessions} sessions x {per_session} turns, main + {extra_branches} branches: \
             seeded in {seeded:.1?} ({} MB); migrated in {took:.1?} \
             ({} branches, {} sessions); the file grew {} MB",
            bytes / 1_000_000,
            report.branches,
            report.sessions,
            after.saturating_sub(bytes) / 1_000_000,
        );
        assert_eq!(report.sessions, sessions);
        assert!(repo.get("main", "/sessions/s00000/turns").is_err());
        assert_eq!(keys(&repo, "s00000").len(), per_session);
    }

    #[test]
    fn the_turns_ref_is_a_local_ref() {
        assert!(is_local_ref(TURNS_REF));
    }
}
