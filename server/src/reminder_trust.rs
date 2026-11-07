//! Which reminders may run their commands on this machine (ctx-hub-sync
//! t-008).
//!
//! A reminder can carry shell commands, and `ctx reminder tick` runs the
//! allow-listed ones of an autonomous reminder. Reminders sync, so without a
//! third gate a reminder made on one machine would run its commands on
//! every machine that pulls it. That gate: a reminder runs here only if it
//! was made or approved through this Hub.
//!
//! The record is kept on [`TRUST_REF`] in the store's default workspace, a
//! local-only ref ([`crate::local_ref`]), at `/reminders/<id>`. Sync never
//! moves it, so a reminder that arrives by a pull isn't trusted here until
//! someone approves it here; what another machine trusts never counts.
//! Schema 4 trusts every reminder a store held before sync existed, since each
//! was made through this Hub.

use std::collections::HashSet;

use agentstategraph::{CommitOptions, RepoError, Repository};
use agentstategraph_core::{IntentCategory, Namespace, Node, Object};
use agentstategraph_reminders::{Reminder, ReminderFilter, ReminderStore};

/// The local-only ref holding the reminders trusted on this machine.
pub const TRUST_REF: &str = "_local/trust";

const ROOT: &str = "reminders";

/// How a reminder came to be trusted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// It was made through this Hub.
    Created,
    /// Someone approved it through this Hub.
    Approved,
    /// The store held it when schema 4 arrived, before sync existed.
    Upgraded,
}

impl How {
    fn as_str(self) -> &'static str {
        match self {
            How::Created => "created",
            How::Approved => "approved",
            How::Upgraded => "upgraded",
        }
    }
}

/// `repo`, or the store's default workspace, where the trust ref lives:
/// reminders belong to the store, not to a workspace.
fn in_default<T>(repo: &Repository, f: impl FnOnce(&Repository) -> T) -> T {
    if repo.namespace().as_str() == Namespace::DEFAULT {
        f(repo)
    } else {
        f(&repo.fork_namespace(Namespace::default_ns()))
    }
}

/// Trust `ids` here. Ids already trusted keep their first record; when
/// nothing is new, nothing is committed.
pub fn record(repo: &Repository, ids: &[&str], how: How, by: &str) -> Result<(), RepoError> {
    in_default(repo, |repo| {
        let held = trusted_in(repo);
        let fresh: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| !id.is_empty() && !held.contains(*id))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        crate::local_ref::ensure(repo, TRUST_REF, &[ROOT])?;
        let at = chrono::Utc::now().to_rfc3339();
        let entry = serde_json::json!({ "how": how.as_str(), "by": by, "at": at });
        let opts =
            |what: String| CommitOptions::new(by, IntentCategory::Custom("Trust".into()), what);
        if let [id] = fresh.as_slice() {
            repo.set_json(
                TRUST_REF,
                &format!("/{ROOT}/{id}"),
                &entry,
                opts(format!("trust reminder {id} here ({})", how.as_str())),
            )?;
            return Ok(());
        }
        // Several at once, in one commit: the map as it is, plus the new ones.
        let mut all = match repo.get_json(TRUST_REF, &format!("/{ROOT}")) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        for id in &fresh {
            all.insert((*id).to_string(), entry.clone());
        }
        repo.set_json(
            TRUST_REF,
            &format!("/{ROOT}"),
            &serde_json::Value::Object(all),
            opts(format!(
                "trust {} reminders here ({})",
                fresh.len(),
                how.as_str()
            )),
        )?;
        Ok(())
    })
}

fn trusted_in(repo: &Repository) -> HashSet<String> {
    match repo.get(TRUST_REF, &format!("/{ROOT}")) {
        Ok(Object::Node(Node::Map(ids))) => ids.into_keys().collect(),
        _ => HashSet::new(),
    }
}

/// The ids of every reminder trusted here. Reads one node.
pub fn trusted(repo: &Repository) -> HashSet<String> {
    in_default(repo, trusted_in)
}

/// Schema 4: trust every reminder the store holds. Before sync, each one was
/// made through this Hub, and a reminder that ran its commands yesterday
/// keeps running them. Returns how many were trusted.
pub fn trust_existing(repo: &Repository) -> Result<usize, RepoError> {
    in_default(repo, |repo| {
        let all = ReminderStore::list(repo, &ReminderFilter::default())
            .map_err(|e| RepoError::InvalidOperation(format!("listing reminders: {e}")))?;
        let ids: Vec<&str> = all.iter().map(|r: &Reminder| r.id.as_str()).collect();
        let before = trusted_in(repo).len();
        record(repo, &ids, How::Upgraded, "ctxone-migration")?;
        Ok(trusted_in(repo).len() - before)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph_reminders::{CreateReminder, ReminderManager};
    use agentstategraph_storage::MemoryStorage;
    use std::sync::Arc;

    fn store() -> Arc<Repository> {
        let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
        repo.init().unwrap();
        repo
    }

    fn reminder(mgr: &ReminderManager, title: &str) -> Reminder {
        mgr.create(CreateReminder::new(
            title.to_string(),
            "do it".to_string(),
            chrono::Utc::now(),
            "test".to_string(),
        ))
        .unwrap()
    }

    #[test]
    fn a_record_is_kept_once_and_never_leaves_the_machine() {
        let repo = store();
        record(&repo, &["r1"], How::Created, "test").unwrap();
        assert!(repo.is_local_only(TRUST_REF).unwrap());
        let head = repo.head(TRUST_REF).unwrap();
        record(&repo, &["r1"], How::Approved, "test").unwrap();
        assert_eq!(repo.head(TRUST_REF).unwrap(), head, "already trusted");
        assert_eq!(
            repo.get_json(TRUST_REF, "/reminders/r1/how").unwrap(),
            "created"
        );
        record(&repo, &["r2", "r3"], How::Approved, "test").unwrap();
        assert_eq!(
            trusted(&repo),
            HashSet::from(["r1".into(), "r2".into(), "r3".into()])
        );
        assert!(
            repo.sync_refs()
                .unwrap()
                .iter()
                .all(|r| r.name != TRUST_REF),
            "a store never offers its trust ref"
        );
    }

    #[test]
    fn a_workspace_reads_and_writes_the_stores_record() {
        let repo = store();
        let ws = repo.fork_namespace(Namespace::new("proj").unwrap());
        ws.init().unwrap();
        record(&ws, &["r1"], How::Created, "test").unwrap();
        assert!(trusted(&repo).contains("r1"));
        assert!(trusted(&ws).contains("r1"));
        assert!(ws.head(TRUST_REF).is_err(), "the record is in default");
    }

    #[test]
    fn the_upgrade_trusts_every_reminder_already_held() {
        let repo = store();
        let mgr = crate::reminder_tools::make_manager(repo.clone());
        let a = reminder(&mgr, "a");
        let b = reminder(&mgr, "b");
        assert_eq!(trust_existing(&repo).unwrap(), 2);
        assert_eq!(trusted(&repo), HashSet::from([a.id.clone(), b.id.clone()]));
        assert_eq!(
            repo.get_json(TRUST_REF, &format!("/reminders/{}/how", a.id))
                .unwrap(),
            "upgraded"
        );
        assert_eq!(trust_existing(&repo).unwrap(), 0);
    }
}
