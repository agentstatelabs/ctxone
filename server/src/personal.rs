//! The personal namespace (ctx-hub-sync t-003).
//!
//! Each store has one, `_personal`, beside its project workspaces. It holds
//! what belongs to the person rather than to a project: the agent rules
//! `ctx agents install` primes, and preferences remembered with
//! `personal: true`. Recall reads it as well as the request's workspace, so a
//! project gets the person's rules without its own copy, and teammates sharing
//! a workspace no longer overwrite each other's.
//!
//! Every namespace is a fork of the same store, so the personal namespace is
//! opened from whatever repository a request already has. It lives on `main`
//! only. Names beginning with `_` are CTXone's: a project can't take one.

use agentstategraph::{RepoError, Repository};
use agentstategraph_core::Namespace;

/// The personal namespace's name.
pub const PERSONAL_NAMESPACE: &str = "_personal";

/// The one branch the personal namespace uses.
pub const PERSONAL_REF: &str = "main";

/// Whether `name` is reserved for CTXone, so no project may use it.
pub fn is_reserved(name: &str) -> bool {
    name.starts_with('_')
}

fn namespace() -> Namespace {
    Namespace::new(PERSONAL_NAMESPACE).expect("`_personal` is a valid namespace name")
}

/// Whether `repo` is the personal namespace itself.
pub fn is_personal(repo: &Repository) -> bool {
    repo.namespace().as_str() == PERSONAL_NAMESPACE
}

/// The personal namespace on `repo`'s store, for reading beside it. `None`
/// when `repo` already is the personal namespace. It may not exist yet, in
/// which case reads of it find nothing.
pub fn beside(repo: &Repository) -> Option<Repository> {
    (!is_personal(repo)).then(|| repo.fork_namespace(namespace()))
}

/// The personal namespace on `repo`'s store, created with its `main` branch
/// if this is its first write.
pub fn open_for_write(repo: &Repository) -> Result<Repository, RepoError> {
    let personal = repo.fork_namespace(namespace());
    personal.init()?;
    Ok(personal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph::CommitOptions;
    use agentstategraph_core::IntentCategory;
    use agentstategraph_storage::SqliteStorage;

    fn store() -> Repository {
        let repo = Repository::new(Box::new(
            SqliteStorage::in_memory().expect("in-memory sqlite"),
        ));
        repo.init().unwrap();
        repo
    }

    #[test]
    fn only_underscore_names_are_reserved() {
        assert!(is_reserved("_personal"));
        assert!(is_reserved("_anything"));
        assert!(!is_reserved("github-com-a-app"));
        assert!(!is_reserved("default"));
    }

    #[test]
    fn a_write_creates_it_and_a_read_beside_a_project_finds_it() {
        let repo = store();
        let project = repo.fork_namespace(Namespace::new("app").unwrap());
        project.init().unwrap();

        // Nothing there before the first write.
        let before = beside(&project).unwrap();
        assert!(before.get_json(PERSONAL_REF, "/memory/x").is_err());

        let personal = open_for_write(&project).unwrap();
        assert!(is_personal(&personal));
        let opts = CommitOptions::new("test", IntentCategory::Checkpoint, "personal write");
        personal
            .set_json(PERSONAL_REF, "/memory/x", &serde_json::json!("mine"), opts)
            .unwrap();

        let read = beside(&project).unwrap();
        assert_eq!(
            read.get_json(PERSONAL_REF, "/memory/x").unwrap(),
            serde_json::json!("mine")
        );
        // The project itself doesn't hold it.
        assert!(project.get_json("main", "/memory/x").is_err());
        // Opening it again is fine, and the personal namespace has nothing beside it.
        assert!(open_for_write(&personal).is_ok());
        assert!(beside(&personal).is_none());
    }
}
