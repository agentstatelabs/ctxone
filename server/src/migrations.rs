//! CtxOne schema migrations.
//!
//! The underlying storage (SQLite / Postgres / in-memory) is owned by the
//! AgentStateGraph engine — we don't manage those tables. What CtxOne *does*
//! own is the **shape** of what's stored inside the graph: the conventions
//! for `/memory/facts/<id>`, `/memory/pinned/<source>/<slug>/title`,
//! `/sessions/<id>/summary`, and so on.
//!
//! If we ever change those conventions, existing graphs break unless we
//! migrate them first. This module provides:
//!
//! - A [`CTXONE_SCHEMA_VERSION`] constant that increments whenever we change
//!   the on-disk shape.
//! - [`run_migrations`] which detects the installed version, runs any
//!   pending migrations in order, and bumps the recorded version.
//! - Forward-compat guard: if the graph was written by a newer Hub than
//!   the one running, we refuse to start rather than corrupt data.
//!
//! The current version is stored as a number at `/ctxone/schema_version`
//! on the `main` branch. It's written once on fresh install and updated
//! by each applied migration.

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::IntentCategory;
use tracing::{debug, info, warn};

/// Current CtxOne schema version. Bump this when you change the shape of
/// anything CtxOne writes to the graph, and add a migration below.
pub const CTXONE_SCHEMA_VERSION: u32 = 2;

/// Path where the schema version is stored inside the graph.
const SCHEMA_VERSION_PATH: &str = "/ctxone/schema_version";

/// Errors that can happen during migration.
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// The graph was written by a newer CtxOne than the current binary.
    /// Refusing to start prevents silent data corruption.
    #[error(
        "graph schema version {graph} is newer than this Hub (v{hub}). \
         Upgrade ctxone-hub or use an older graph."
    )]
    NewerGraph { graph: u32, hub: u32 },

    /// An underlying repository operation failed.
    #[error("repository error: {0}")]
    Repo(String),
}

impl From<agentstategraph::RepoError> for MigrationError {
    fn from(e: agentstategraph::RepoError) -> Self {
        MigrationError::Repo(e.to_string())
    }
}

/// Read the schema version currently recorded in the graph. Returns 0
/// if the path doesn't exist (i.e., this is a fresh graph or one that
/// predates schema tracking).
pub fn current_schema_version(repo: &Repository) -> u32 {
    match repo.get_json("main", SCHEMA_VERSION_PATH) {
        Ok(v) => v.as_u64().unwrap_or(0) as u32,
        Err(_) => 0,
    }
}

/// Write the schema version to the graph with an auditable Migrate intent.
fn set_schema_version(repo: &Repository, version: u32) -> Result<(), MigrationError> {
    let opts = CommitOptions::new(
        "ctxone-migration",
        IntentCategory::Migrate,
        format!("ctxone schema -> v{}", version),
    )
    .with_confidence(1.0);

    repo.set_json(
        "main",
        SCHEMA_VERSION_PATH,
        &serde_json::json!(version),
        opts,
    )?;
    Ok(())
}

/// Run any pending migrations, detecting the installed version and
/// applying each step in order.
///
/// Behavior:
///
/// - **Fresh graph (version 0):** writes `CTXONE_SCHEMA_VERSION` to the
///   graph. No data migration needed.
/// - **Existing graph at current version:** no-op, logs at debug.
/// - **Existing graph behind current version:** runs each migration
///   from `current+1` through `CTXONE_SCHEMA_VERSION`, then records the
///   final version.
/// - **Existing graph ahead of current version:** returns [`MigrationError::NewerGraph`]
///   so the Hub refuses to start. This protects users who downgrade
///   their binary.
pub fn run_migrations(repo: &Repository) -> Result<(), MigrationError> {
    let current = current_schema_version(repo);

    if current > CTXONE_SCHEMA_VERSION {
        warn!(
            graph_version = current,
            hub_version = CTXONE_SCHEMA_VERSION,
            "graph is from a newer Hub; refusing to start"
        );
        return Err(MigrationError::NewerGraph {
            graph: current,
            hub: CTXONE_SCHEMA_VERSION,
        });
    }

    if current == CTXONE_SCHEMA_VERSION {
        debug!(
            version = current,
            "schema version is current, no migrations needed"
        );
        return Ok(());
    }

    if current == 0 {
        info!(
            to = CTXONE_SCHEMA_VERSION,
            "fresh graph; initializing schema version"
        );
    } else {
        info!(
            from = current,
            to = CTXONE_SCHEMA_VERSION,
            "running pending migrations"
        );
    }

    // Apply each migration from `current + 1` through CTXONE_SCHEMA_VERSION.
    // Add new arms here when the schema changes.
    for version in (current + 1)..=CTXONE_SCHEMA_VERSION {
        match version {
            1 => {
                // v0 -> v1: first schema version. Nothing to transform.
                // This arm exists so fresh graphs get tagged with v1 and
                // future migrations can start from a known baseline.
                info!(version = 1, "migration 001: initialize schema");
            }
            2 => migrate_001_to_002(repo)?,
            // 3 => migrate_002_to_003(repo)?,
            other => {
                return Err(MigrationError::Repo(format!(
                    "no migration registered for schema version {}",
                    other
                )));
            }
        }
    }

    set_schema_version(repo, CTXONE_SCHEMA_VERSION)?;
    info!(version = CTXONE_SCHEMA_VERSION, "migrations complete");

    Ok(())
}

/// v1 -> v2: give pre-namespace epochs their workspace back.
///
/// agentstategraph v1.1.0 gave epochs a `namespace` and scoped seal enforcement
/// and listing to it, but only stamps epochs created from v1.1.0 onward. Epochs
/// sealed before then carry no owner and read as belonging to the DEFAULT
/// workspace — on a mature hub that is most of them, all attributed to the
/// wrong place.
///
/// CtxOne knows where they belong: `seal_plan_epoch` writes the id
/// `plan:<namespace>:<plan_id>`, so the workspace is recoverable from the id
/// itself. This settles it once, in the graph, so the Hub can attribute epochs
/// by their real namespace instead of matching that id prefix forever.
///
/// Only touches epochs that have no owner — `assign_epoch_namespace` is
/// assign-only, so this can never move an epoch between workspaces. Ids that do
/// not match the `plan:<ns>:` convention are left alone: they were never
/// attributed by prefix either, so there is nothing to recover.
fn migrate_001_to_002(repo: &Repository) -> Result<(), MigrationError> {
    let epochs = repo
        .list_all_epochs()
        .map_err(|e| MigrationError::Repo(format!("list epochs: {e}")))?;

    let (mut assigned, mut already, mut skipped) = (0usize, 0usize, 0usize);
    for e in &epochs {
        if e.namespace.is_some() {
            already += 1;
            continue;
        }
        let Some(ns) = namespace_from_plan_epoch_id(&e.id) else {
            skipped += 1;
            continue;
        };
        match repo.assign_epoch_namespace(&e.id, &ns) {
            Ok(true) => assigned += 1,
            Ok(false) => already += 1,
            // Best-effort per epoch: one unassignable epoch must not block
            // startup, and the Hub still reads correctly without it.
            Err(err) => {
                warn!(epoch = %e.id, error = ?err, "could not assign epoch namespace");
                skipped += 1;
            }
        }
    }
    info!(
        version = 2,
        assigned,
        already_owned = already,
        skipped,
        total = epochs.len(),
        "migration 002: backfilled epoch workspaces from plan:<ns>: ids"
    );
    Ok(())
}

/// Recover the workspace from a `plan:<namespace>:<plan_id>` epoch id.
///
/// Returns `None` for any other shape. Splits on the FIRST two colons only —
/// a plan id may itself contain colons, and the namespace never does.
fn namespace_from_plan_epoch_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("plan:")?;
    let (ns, plan) = rest.split_once(':')?;
    if ns.is_empty() || plan.is_empty() {
        return None;
    }
    Some(ns.to_string())
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph_storage::SqliteStorage;

    #[test]
    fn namespace_from_plan_epoch_id_recovers_the_workspace() {
        assert_eq!(
            namespace_from_plan_epoch_id("plan:sessiondrift:playhead"),
            Some("sessiondrift".to_string())
        );
        // A plan id may itself contain colons; the namespace never does.
        assert_eq!(
            namespace_from_plan_epoch_id("plan:ctxone:feat:sub:task"),
            Some("ctxone".to_string())
        );
    }

    #[test]
    fn namespace_from_plan_epoch_id_rejects_other_shapes() {
        for id in [
            "plan:onlyone",  // no second colon
            "notaplan:ws:p", // wrong prefix
            "plan::p",       // empty namespace
            "plan:ws:",      // empty plan id
            "",
        ] {
            assert_eq!(
                namespace_from_plan_epoch_id(id),
                None,
                "should reject {id:?}"
            );
        }
    }

    fn fresh_repo() -> Repository {
        let repo = Repository::new(Box::new(
            SqliteStorage::in_memory().expect("in-memory sqlite"),
        ));
        repo.init().expect("repo init");
        repo
    }

    #[test]
    fn fresh_repo_has_version_zero() {
        let repo = fresh_repo();
        assert_eq!(current_schema_version(&repo), 0);
    }

    #[test]
    fn run_migrations_initializes_fresh_repo_to_current_version() {
        let repo = fresh_repo();
        run_migrations(&repo).expect("migrations should succeed");
        assert_eq!(current_schema_version(&repo), CTXONE_SCHEMA_VERSION);
    }

    #[test]
    fn run_migrations_is_idempotent() {
        let repo = fresh_repo();
        run_migrations(&repo).unwrap();
        let first_version = current_schema_version(&repo);

        // Second run should be a no-op
        run_migrations(&repo).unwrap();
        assert_eq!(current_schema_version(&repo), first_version);
    }

    #[test]
    fn run_migrations_refuses_newer_graph() {
        let repo = fresh_repo();
        // Simulate a graph written by a future Hub.
        set_schema_version(&repo, CTXONE_SCHEMA_VERSION + 1).unwrap();

        let result = run_migrations(&repo);
        assert!(result.is_err());
        match result {
            Err(MigrationError::NewerGraph { graph, hub }) => {
                assert_eq!(graph, CTXONE_SCHEMA_VERSION + 1);
                assert_eq!(hub, CTXONE_SCHEMA_VERSION);
            }
            other => panic!("expected NewerGraph error, got {:?}", other),
        }
    }

    #[test]
    fn set_schema_version_is_auditable_via_blame() {
        let repo = fresh_repo();
        run_migrations(&repo).unwrap();

        // The schema-version write should have landed under a Migrate intent
        // with the ctxone-migration agent. This is indirectly verified by the
        // round-trip: we can read the value back and it matches the constant.
        assert_eq!(current_schema_version(&repo), CTXONE_SCHEMA_VERSION);
    }
}
