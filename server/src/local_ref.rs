//! Refs that never leave this machine (ctx-hub-sync t-002).
//!
//! A local-only ref is an ordinary AgentStateGraph branch marked with
//! `Repository::set_local_only`: AgentStateGraph never pushes it, and a Hub
//! never offers it to a fetch. [`crate::turns::TURNS_REF`] holds a
//! workspace's raw session turns.
//!
//! Their names begin with [`LOCAL_REF_PREFIX`]. Branch listings leave them
//! out, and no tool creates, merges, resets or deletes one by name: merging
//! one into a branch would put what it holds where it syncs.

use std::collections::BTreeMap;
use std::sync::Mutex;

use agentstategraph::{CommitOptions, RepoError, Repository};
use agentstategraph_core::{IntentCategory, Node, Object, ObjectId};

/// The prefix every local-only ref's name begins with.
pub const LOCAL_REF_PREFIX: &str = "_local/";

/// Whether `name` is a local-only ref CTXone manages, which no tool may take
/// as a branch name.
pub fn is_local_ref(name: &str) -> bool {
    name.starts_with(LOCAL_REF_PREFIX)
}

/// The error a tool returns for a [`LOCAL_REF_PREFIX`] name.
pub fn refused(name: &str) -> String {
    format!(
        "{name} is a local-only ref CTXone keeps on this machine (refs beginning with \
         {LOCAL_REF_PREFIX} never sync); it can't be created, merged, reset or deleted by name"
    )
}

/// Serializes creating local-only refs within this process.
static CREATING: Mutex<()> = Mutex::new(());

/// Create the local-only ref `name` if it's missing or unfinished: branched
/// from `main`, marked local-only, then cut down so its root holds `/_meta`
/// (the mark) and the top-level keys in `keep`, nothing of `main`'s. The
/// workspace's `main` must exist.
pub fn ensure(repo: &Repository, name: &str, keep: &[&str]) -> Result<(), RepoError> {
    debug_assert!(is_local_ref(name));
    if ready(repo, name, keep) {
        return Ok(());
    }
    let _guard = CREATING.lock().unwrap_or_else(|e| e.into_inner());
    match repo.branch(name, "main") {
        Ok(_) | Err(RepoError::BranchAlreadyExists(_)) => {}
        Err(e) => return Err(e),
    }
    if !repo.is_local_only(name)? {
        repo.set_local_only(name, true)?;
    }
    if let Object::Node(Node::Map(root)) = repo.get(name, "/")? {
        let kept: BTreeMap<String, ObjectId> = root
            .iter()
            .filter(|(key, _)| kept(key, keep))
            .map(|(key, id)| (key.clone(), *id))
            .collect();
        if kept.len() != root.len() {
            repo.set(
                name,
                "/",
                &Object::Node(Node::Map(kept)),
                CommitOptions::new(
                    "ctxone",
                    IntentCategory::Migrate,
                    format!("keep only what {name} is for"),
                ),
            )?;
        }
    }
    Ok(())
}

fn kept(key: &str, keep: &[&str]) -> bool {
    key == "_meta" || keep.contains(&key)
}

/// Whether `name` exists, is marked local-only and holds only what it
/// should. A run that stopped part way through [`ensure`] isn't ready.
fn ready(repo: &Repository, name: &str, keep: &[&str]) -> bool {
    if repo.head(name).is_err() || !repo.is_local_only(name).unwrap_or(false) {
        return false;
    }
    match repo.get(name, "/") {
        Ok(Object::Node(Node::Map(root))) => root.keys().all(|key| kept(key, keep)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph_storage::MemoryStorage;

    #[test]
    fn local_refs_are_recognised() {
        assert!(is_local_ref("_local/turns"));
        assert!(is_local_ref("_local/anything"));
        assert!(!is_local_ref("main"));
        assert!(!is_local_ref("local/turns"));
    }

    #[test]
    fn ensure_makes_a_marked_ref_holding_none_of_main() {
        let repo = Repository::new(Box::new(MemoryStorage::new()));
        repo.init().unwrap();
        let opts = || CommitOptions::new("t", IntentCategory::Checkpoint, "seed");
        repo.set_json("main", "/memory/f", &serde_json::json!("x"), opts())
            .unwrap();
        ensure(&repo, "_local/x", &["things"]).unwrap();
        assert!(repo.is_local_only("_local/x").unwrap());
        assert!(repo.get("_local/x", "/memory").is_err());
        let head = repo.head("_local/x").unwrap();
        ensure(&repo, "_local/x", &["things"]).unwrap();
        assert_eq!(
            repo.head("_local/x").unwrap(),
            head,
            "a second call is a no-op"
        );
    }
}
