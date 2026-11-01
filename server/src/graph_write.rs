//! Writes that skip a commit when nothing would change (ctx-hub-sync t-004).
//!
//! AgentStateGraph commits every `set_json`, even one that leaves the tree as
//! it was. On a measured store a third of all commits were like that:
//! re-imports, a grown session's unchanged title, provenance and burn, its
//! turns written again as they were, and sections primed again unchanged.
//! Each one grows history and, once workspaces sync, what a sync moves.
//!
//! Values are content-addressed, so whether a write would change anything is
//! one comparison: the id of the object at the path against the id the value
//! would get. The value's id comes from AgentStateGraph's own
//! [`tree::json_to_tree`], run against a throwaway in-memory store, so it's the
//! id a real write would store.

use agentstategraph::{CommitOptions, RepoError, Repository, tree};
use agentstategraph_core::ObjectId;
use agentstategraph_storage::MemoryStorage;

/// Whether `path` on `ref_name` already holds exactly `value`. False when
/// either side can't be read, so a doubtful case writes.
pub fn holds(repo: &Repository, ref_name: &str, path: &str, value: &serde_json::Value) -> bool {
    let Ok(current) = repo.get(ref_name, path) else {
        return false;
    };
    match tree::json_to_tree(&MemoryStorage::new(), value) {
        Ok(id) => current.id() == id,
        Err(_) => false,
    }
}

/// [`Repository::set_json`], unless `path` already holds `value`. Then nothing
/// is committed, and the ref's head, where the value already is, comes back in
/// place of a new commit.
///
/// Skipped with the commit are what a write runs after it, such as watch
/// escalation, and what it checks before, such as a taint on the path: the
/// value there is already the one being written.
pub fn set_json_if_changed(
    repo: &Repository,
    ref_name: &str,
    path: &str,
    value: &serde_json::Value,
    opts: CommitOptions,
) -> Result<ObjectId, RepoError> {
    if holds(repo, ref_name, path, value) {
        return repo.head(ref_name);
    }
    repo.set_json(ref_name, path, value, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph_core::IntentCategory;
    use serde_json::json;

    fn repo() -> Repository {
        let repo = Repository::new(Box::new(MemoryStorage::new()));
        repo.init().expect("init");
        repo
    }

    fn opts() -> CommitOptions {
        CommitOptions::new("test", IntentCategory::Custom("Observe".into()), "write")
    }

    fn commits(repo: &Repository) -> usize {
        repo.log("main", 1000).expect("log").len()
    }

    #[test]
    fn an_unchanged_value_writes_no_commit() {
        let repo = repo();
        let value = json!({ "title": "t", "turns": [1, 2, { "a": null }] });
        let first = set_json_if_changed(&repo, "main", "/sessions/s/meta", &value, opts()).unwrap();
        let before = commits(&repo);

        let again = set_json_if_changed(&repo, "main", "/sessions/s/meta", &value, opts()).unwrap();
        assert_eq!(commits(&repo), before, "no-op write committed");
        assert_eq!(again, first, "an unchanged write answers with the head");
        assert_eq!(repo.get_json("main", "/sessions/s/meta").unwrap(), value);
    }

    #[test]
    fn a_changed_value_or_new_path_commits() {
        let repo = repo();
        set_json_if_changed(&repo, "main", "/a", &json!(1), opts()).unwrap();
        let before = commits(&repo);

        set_json_if_changed(&repo, "main", "/a", &json!(2), opts()).unwrap();
        set_json_if_changed(&repo, "main", "/b", &json!(2), opts()).unwrap();
        // Same JSON number, different type: written, as a plain write would.
        set_json_if_changed(&repo, "main", "/a", &json!(2.0), opts()).unwrap();
        assert_eq!(commits(&repo), before + 3);
        assert_eq!(repo.get_json("main", "/a").unwrap(), json!(2.0));
    }

    #[test]
    fn holds_is_false_for_a_missing_path_or_ref() {
        let repo = repo();
        assert!(!holds(&repo, "main", "/nothing", &json!(null)));
        assert!(!holds(&repo, "no-such-ref", "/a", &json!(1)));
    }
}
