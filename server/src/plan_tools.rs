//! CTXone plan tools — wrap `agentstategraph-tasks` and surface it as
//! MCP tools + shared helpers for the HTTP endpoints.
//!
//! The substrate crate (`agentstategraph_tasks::TaskStore`) handles
//! plan storage, state transitions, blocker validation, parent rollup,
//! proof storage, and plan completion. CTXone adds:
//!
//! - **MCP tool surface** (`plan_new`, `plan_add`, `plan_start`,
//!   `plan_complete`, `plan_abandon`, `plan_next`, `plan_list`,
//!   `plan_get`, `plan_archive`, `plan_tasks`) with proactive
//!   "CALL THIS WHEN" descriptions. The tool methods themselves live
//!   on `CtxOneServer` in `memory_tools.rs` so every MCP tool shares a
//!   single `ToolRouter`; this module provides the typed parameter
//!   structs and the pure helpers the tool bodies delegate into.
//! - **`assigned_to` sidecar** — the substrate's `Task` struct does
//!   not carry an `assigned_to` field in the v0.4 shape we depend on,
//!   so CTXone stores assignments as a separate JSON map under
//!   `/plans/<plan>/_assignments`. `next_pickable_task` filters against
//!   this map when an `assigned_to` argument is supplied, which is what
//!   unlocks the state-driven orchestration pattern across multiple
//!   agents sharing a plan.
//! - **Response enrichment** — `task_to_json` stitches in `assigned_to`
//!   from the sidecar so callers never need to know about the split.

use std::sync::Arc;

use agentstategraph::{CommitOptions, Repository};
use agentstategraph_core::IntentCategory;
use agentstategraph_tasks::{
    Plan, PlanStatus, Priority, Proof, ProofKind, Task, TaskId, TaskStatus, TaskStore,
    TaskStoreError,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Default path prefix where plans live in the state graph.
pub const PLANS_PREFIX: &str = "/plans";

/// Top-level path prefix where per-task `assigned_to` values are stored.
///
/// The substrate's `TaskStore::list_tasks` treats every non-`_meta` key
/// under a plan root as a `Task`, so we can't colocate the assignment
/// map inside `/plans/<plan>/`. Instead we store assignments under
/// `/plan_assignments/<plan>` as a JSON object keyed by task id.
/// This also keeps plan data and assignment data cleanly separable if
/// the substrate grows a native `assigned_to` field later.
pub const ASSIGNMENTS_PREFIX: &str = "/plan_assignments";

fn default_ref() -> String {
    "main".to_string()
}

fn default_true() -> bool {
    true
}

pub fn priority_from_str(s: &str) -> Option<Priority> {
    match s.to_ascii_lowercase().as_str() {
        "low" => Some(Priority::Low),
        "medium" => Some(Priority::Medium),
        "high" => Some(Priority::High),
        "critical" => Some(Priority::Critical),
        _ => None,
    }
}

pub fn priority_label(p: Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
        Priority::Critical => "critical",
    }
}

pub fn task_status_label(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Pending => "pending",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::Abandoned => "abandoned",
    }
}

pub fn plan_status_label(s: PlanStatus) -> &'static str {
    match s {
        PlanStatus::Active => "active",
        PlanStatus::Completed => "completed",
        PlanStatus::Archived => "archived",
    }
}

pub fn proof_kind_label(k: ProofKind) -> &'static str {
    match k {
        ProofKind::Commit => "commit",
        ProofKind::File => "file",
        ProofKind::Test => "test",
        ProofKind::Text => "text",
    }
}

pub fn plan_status_from_str(s: &str) -> Option<PlanStatus> {
    match s.to_ascii_lowercase().as_str() {
        "active" => Some(PlanStatus::Active),
        "completed" => Some(PlanStatus::Completed),
        "archived" => Some(PlanStatus::Archived),
        _ => None,
    }
}

/// Build a `Proof` from `(kind, value, note?)`.
pub fn parse_proof(
    kind: &str,
    value: &str,
    note: Option<String>,
) -> Result<Proof, PlanToolError> {
    if value.trim().is_empty() {
        return Err(PlanToolError::InvalidProof(
            "proof value must be non-empty".into(),
        ));
    }
    let p = match kind.to_ascii_lowercase().as_str() {
        "commit" => Proof::commit(value),
        "file" => Proof::file(value),
        "test" => Proof::test(value),
        "text" => Proof::text(value),
        other => {
            return Err(PlanToolError::InvalidProof(format!(
                "unknown proof kind '{}' (expected commit|file|test|text)",
                other
            )));
        }
    };
    Ok(match note {
        Some(n) if !n.is_empty() => p.with_note(n),
        _ => p,
    })
}

/// Errors surfaced from the plan-tool layer. Wraps substrate errors
/// but adds an `InvalidProof` variant and friendlier string coercion.
#[derive(Debug, thiserror::Error)]
pub enum PlanToolError {
    #[error("{0}")]
    Substrate(#[from] TaskStoreError),

    #[error("invalid proof: {0}")]
    InvalidProof(String),

    #[error("invalid priority: {0}")]
    InvalidPriority(String),

    #[error("repository error: {0}")]
    Repo(String),
}

impl From<agentstategraph::RepoError> for PlanToolError {
    fn from(e: agentstategraph::RepoError) -> Self {
        PlanToolError::Repo(e.to_string())
    }
}

/// Build a `TaskStore` bound to the default plans prefix for the
/// given agent. Use this inside every tool/endpoint so the prefix is
/// consistent across the Hub.
pub fn make_store(repo: Arc<Repository>, agent_id: &str) -> TaskStore {
    TaskStore::new(repo, PLANS_PREFIX, agent_id)
}

// -- assigned_to sidecar helpers -------------------------------------

fn assignments_path(plan: &str) -> String {
    format!("{}/{}", ASSIGNMENTS_PREFIX, plan)
}

/// Read the assigned-to map for a plan. Missing map returns empty.
pub fn read_assignments(
    repo: &Repository,
    ref_name: &str,
    plan: &str,
) -> std::collections::BTreeMap<String, String> {
    let path = assignments_path(plan);
    match repo.get_json(ref_name, &path) {
        Ok(serde_json::Value::Object(map)) => map
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect(),
        _ => std::collections::BTreeMap::new(),
    }
}

/// Merge an `assigned_to` value for a single task into the sidecar
/// map. If `agent` is `None`, the entry is cleared.
pub fn set_assignment(
    repo: &Repository,
    ref_name: &str,
    plan: &str,
    task_id: &TaskId,
    agent: Option<&str>,
    author: &str,
) -> Result<(), PlanToolError> {
    let mut map = read_assignments(repo, ref_name, plan);
    match agent {
        Some(a) if !a.is_empty() => {
            map.insert(task_id.as_str().to_string(), a.to_string());
        }
        _ => {
            map.remove(task_id.as_str());
        }
    }

    let path = assignments_path(plan);
    let value = serde_json::to_value(&map)
        .map_err(|e| PlanToolError::Repo(format!("serialize assignments: {}", e)))?;
    let opts = CommitOptions::new(
        author,
        IntentCategory::Plan,
        format!("Assign task {} on plan {}", task_id, plan),
    );
    repo.set_json(ref_name, &path, &value, opts)?;
    Ok(())
}

/// Render a task as a JSON object, stitching in `assigned_to` from
/// the sidecar. This is the shape the Hub exposes on the wire.
pub fn task_to_json(
    task: &Task,
    assigned_to: Option<&str>,
) -> serde_json::Value {
    let mut out = serde_json::json!({
        "id": task.id.as_str(),
        "title": task.title,
        "status": task_status_label(task.status),
        "priority": priority_label(task.priority),
        "parent_id": task.parent_id.as_ref().map(|t| t.as_str()),
        "blocked_by": task
            .blocked_by
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>(),
        "assigned_to": assigned_to,
        "created_at": task.created_at.to_rfc3339(),
        "created_by": task.created_by,
        "started_at": task.started_at.map(|t| t.to_rfc3339()),
        "started_by": task.started_by,
        "completed_at": task.completed_at.map(|t| t.to_rfc3339()),
        "completed_by": task.completed_by,
        "abandoned_at": task.abandoned_at.map(|t| t.to_rfc3339()),
        "abandoned_reason": task.abandoned_reason,
    });
    if let Some(p) = &task.proof {
        out["proof"] = serde_json::json!({
            "kind": proof_kind_label(p.kind),
            "value": p.value,
            "note": p.note,
        });
    } else {
        out["proof"] = serde_json::Value::Null;
    }
    out
}

/// Render a plan as a JSON object, including task counts keyed by
/// status. The `tasks` field is included only when `with_tasks` is
/// true — the list endpoint keeps responses short, the detail endpoint
/// opts in.
pub fn plan_to_json(
    plan: &Plan,
    tasks: &[Task],
    assignments: &std::collections::BTreeMap<String, String>,
    with_tasks: bool,
) -> serde_json::Value {
    let mut pending = 0u32;
    let mut in_progress = 0u32;
    let mut done = 0u32;
    let mut abandoned = 0u32;
    for t in tasks {
        match t.status {
            TaskStatus::Pending => pending += 1,
            TaskStatus::InProgress => in_progress += 1,
            TaskStatus::Done => done += 1,
            TaskStatus::Abandoned => abandoned += 1,
        }
    }
    let task_counts = serde_json::json!({
        "pending": pending,
        "in_progress": in_progress,
        "done": done,
        "abandoned": abandoned,
        "total": tasks.len() as u32,
    });

    let mut out = serde_json::json!({
        "name": plan.name,
        "description": plan.description,
        "status": plan_status_label(plan.status),
        "created_at": plan.created_at.to_rfc3339(),
        "created_by": plan.created_by,
        "archived_at": plan.archived_at.map(|t| t.to_rfc3339()),
        "task_counts": task_counts,
    });

    if with_tasks {
        let tasks_json: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| task_to_json(t, assignments.get(t.id.as_str()).map(|s| s.as_str())))
            .collect();
        out["tasks"] = serde_json::Value::Array(tasks_json);
    }

    out
}

// -- Filter helpers used by both MCP and HTTP ------------------------

/// Next-task semantics: find the highest-priority pickable task,
/// optionally filtered by `assigned_to`.
///
/// - `assigned_to = None`: return whatever the substrate picks (any
///   pending task, blockers satisfied).
/// - `assigned_to = Some(agent)`, `include_unassigned = true`,
///   `assigned_only = false`: return the best pending task that is
///   either assigned to `agent` or unassigned.
/// - `assigned_to = Some(agent)`, `assigned_only = true`: return only
///   tasks explicitly assigned to `agent`.
/// - `assigned_to = Some(agent)`, `include_unassigned = false`,
///   `assigned_only = false`: tasks assigned to `agent` only (alias).
pub fn next_pickable_task(
    store: &TaskStore,
    repo: &Repository,
    ref_name: &str,
    plan: &str,
    assigned_to: Option<&str>,
    include_unassigned: bool,
    assigned_only: bool,
) -> Result<Option<Task>, PlanToolError> {
    let tasks = store.list_tasks(ref_name, plan)?;
    let assignments = read_assignments(repo, ref_name, plan);

    let mut candidates: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .filter(|t| {
            t.blocked_by.iter().all(|b| {
                tasks
                    .iter()
                    .any(|other| &other.id == b && other.status == TaskStatus::Done)
            })
        })
        .filter(|t| {
            let assigned = assignments.get(t.id.as_str()).map(|s| s.as_str());
            match assigned_to {
                None => true,
                Some(agent) => match assigned {
                    Some(a) if a == agent => true,
                    Some(_) => false, // assigned to someone else
                    None => !assigned_only && include_unassigned,
                },
            }
        })
        .collect();
    candidates.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
    Ok(candidates.first().map(|t| (*t).clone()))
}

// -- MCP / HTTP parameter types -------------------------------------

#[derive(Deserialize, JsonSchema)]
pub struct PlanNewParams {
    /// Name of the plan (kebab-case, no spaces). Used in paths.
    pub name: String,
    /// Optional one or two sentence description.
    pub description: Option<String>,
    /// Branch to write to (default: "main").
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanAddParams {
    /// Plan name to add the task to.
    pub plan_id: String,
    /// Task title (imperative sentence, one line).
    pub title: String,
    /// Optional longer-form task description. Stored in the title for now —
    /// the substrate's Task shape does not carry a description field, so
    /// if present the description is appended after an em dash.
    pub description: Option<String>,
    /// Priority: low | medium | high | critical. Default: medium.
    pub priority: Option<String>,
    /// Parent task id for a subtask. Substrate enforces exactly two levels.
    pub parent_id: Option<String>,
    /// Agent ID this task is intended for (e.g. "claude-code", "codex",
    /// a user email). Enables state-driven orchestration when paired with
    /// `plan_next(assigned_to=...)`. Free-form — any string works.
    pub assigned_to: Option<String>,
    /// Task ids that must be `done` before this task can be started.
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanStartParams {
    pub plan_id: String,
    pub task_id: String,
    /// Optional reason recorded in blame for the transition.
    pub reason: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanCompleteParams {
    pub plan_id: String,
    pub task_id: String,
    /// Proof object: `{kind, value, note?}`. `kind` is one of
    /// commit|file|test|text. Prefer `commit` when available, then
    /// `file`, then `test`, then `text` as a last resort.
    pub proof: ProofParam,
    pub reason: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProofParam {
    /// commit | file | test | text
    pub kind: String,
    /// The SHA, path, test name, or free-form note.
    pub value: String,
    /// Optional human-readable note.
    pub note: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanAbandonParams {
    pub plan_id: String,
    pub task_id: String,
    /// Required reason — abandon without a reason is rejected.
    pub reason: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanNextParams {
    pub plan_id: String,
    /// Filter to tasks assigned to this agent. Pass "me" to resolve to
    /// the MCP server's own agent id (set via `--agent-id` or the
    /// `X-CTXone-Agent` header).
    pub assigned_to: Option<String>,
    /// When true (default), include unassigned tasks alongside ones
    /// assigned to `assigned_to`.
    #[serde(default = "default_true")]
    pub include_unassigned: bool,
    /// When true, return only tasks explicitly assigned to `assigned_to`.
    #[serde(default)]
    pub assigned_only: bool,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanListParams {
    /// Filter by plan status (active|completed|archived). Omit for all.
    pub status_filter: Option<String>,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanGetParams {
    pub plan_id: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanArchiveParams {
    pub plan_id: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PlanTasksParams {
    pub plan_id: String,
    #[serde(default = "default_ref", rename = "ref")]
    pub ref_name: String,
}

#[derive(Serialize)]
pub struct ToolErr {
    pub error: String,
}

pub fn err_json(e: impl std::fmt::Display) -> String {
    serde_json::to_string(&ToolErr {
        error: e.to_string(),
    })
    .unwrap_or_else(|_| "{\"error\":\"unknown\"}".to_string())
}

// -- Top-level plan operations usable by both MCP and HTTP -----------

/// Create a plan. Writes `/plans/<name>/_meta` via the substrate.
pub fn create_plan(
    store: &TaskStore,
    ref_name: &str,
    name: &str,
    description: Option<String>,
) -> Result<Plan, PlanToolError> {
    let p = store.create_plan(ref_name, name, description)?;
    Ok(p)
}

/// Add a task, with optional assigned_to sidecar write.
#[allow(clippy::too_many_arguments)]
pub fn add_task(
    store: &TaskStore,
    repo: &Repository,
    ref_name: &str,
    plan: &str,
    title: &str,
    description: Option<&str>,
    priority: Option<&str>,
    parent_id: Option<&str>,
    assigned_to: Option<&str>,
    blocked_by: Vec<String>,
    author: &str,
) -> Result<(Task, Option<String>), PlanToolError> {
    let pri = match priority {
        None => Priority::Medium,
        Some(s) => priority_from_str(s)
            .ok_or_else(|| PlanToolError::InvalidPriority(s.to_string()))?,
    };

    let parent = parent_id.map(|s| TaskId(s.to_string()));
    let blockers: Vec<TaskId> = blocked_by.into_iter().map(TaskId).collect();

    // Append description into title if present — we don't have a
    // description field on Task in the substrate. Keep it compact.
    let full_title = match description {
        Some(d) if !d.is_empty() => format!("{} — {}", title, d),
        _ => title.to_string(),
    };

    let task = store.add_task(ref_name, plan, &full_title, pri, parent, blockers)?;
    if let Some(a) = assigned_to
        && !a.is_empty()
    {
        set_assignment(repo, ref_name, plan, &task.id, Some(a), author)?;
    }
    Ok((task, assigned_to.map(|s| s.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph::Repository;
    use agentstategraph_storage::MemoryStorage;

    fn fresh_repo() -> Arc<Repository> {
        let repo = Arc::new(Repository::new(Box::new(MemoryStorage::new())));
        repo.init().expect("repo init");
        repo
    }

    fn fresh_store() -> (Arc<Repository>, TaskStore) {
        let repo = fresh_repo();
        let store = make_store(repo.clone(), "test-agent");
        (repo, store)
    }

    #[test]
    fn parse_proof_commit() {
        let p = parse_proof("commit", "abc123", None).unwrap();
        assert_eq!(p.kind, ProofKind::Commit);
        assert_eq!(p.value, "abc123");
        assert!(p.note.is_none());
    }

    #[test]
    fn parse_proof_with_note() {
        let p = parse_proof("file", "src/foo.rs", Some("notes".into())).unwrap();
        assert_eq!(p.kind, ProofKind::File);
        assert_eq!(p.note.as_deref(), Some("notes"));
    }

    #[test]
    fn parse_proof_rejects_empty_value() {
        assert!(parse_proof("commit", "", None).is_err());
    }

    #[test]
    fn parse_proof_rejects_unknown_kind() {
        assert!(parse_proof("screenshot", "foo", None).is_err());
    }

    #[test]
    fn priority_from_str_parses_known() {
        assert_eq!(priority_from_str("low"), Some(Priority::Low));
        assert_eq!(priority_from_str("MEDIUM"), Some(Priority::Medium));
        assert_eq!(priority_from_str("High"), Some(Priority::High));
        assert_eq!(priority_from_str("critical"), Some(Priority::Critical));
        assert_eq!(priority_from_str("nope"), None);
    }

    #[test]
    fn create_plan_round_trip() {
        let (_repo, store) = fresh_store();
        let plan = create_plan(&store, "main", "p1", Some("desc".into())).unwrap();
        assert_eq!(plan.name, "p1");
        assert_eq!(plan.status, PlanStatus::Active);
        let fetched = store.get_plan("main", "p1").unwrap();
        assert_eq!(plan, fetched);
    }

    #[test]
    fn add_task_stores_assignment_in_sidecar() {
        let (repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let (task, assigned) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "Do the thing",
            None,
            Some("high"),
            None,
            Some("codex"),
            vec![],
            "test-agent",
        )
        .unwrap();
        assert_eq!(task.priority, Priority::High);
        assert_eq!(assigned.as_deref(), Some("codex"));

        let map = read_assignments(&repo, "main", "p1");
        assert_eq!(map.get(task.id.as_str()).map(|s| s.as_str()), Some("codex"));
    }

    #[test]
    fn add_task_without_assignment_leaves_sidecar_empty() {
        let (repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let (task, assigned) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "Do the thing",
            None,
            None,
            None,
            None,
            vec![],
            "test-agent",
        )
        .unwrap();
        assert!(assigned.is_none());

        let map = read_assignments(&repo, "main", "p1");
        assert!(!map.contains_key(task.id.as_str()));
    }

    #[test]
    fn next_pickable_task_filters_by_assignee() {
        let (repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let (_t1, _) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "for codex",
            None,
            Some("high"),
            None,
            Some("codex"),
            vec![],
            "test-agent",
        )
        .unwrap();
        let (_t2, _) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "for claude",
            None,
            Some("medium"),
            None,
            Some("claude-code"),
            vec![],
            "test-agent",
        )
        .unwrap();
        let (_t3, _) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "unassigned",
            None,
            Some("low"),
            None,
            None,
            vec![],
            "test-agent",
        )
        .unwrap();

        let next = next_pickable_task(&store, &repo, "main", "p1", Some("codex"), true, false)
            .unwrap()
            .unwrap();
        assert_eq!(next.title, "for codex");

        let next =
            next_pickable_task(&store, &repo, "main", "p1", Some("claude-code"), true, false)
                .unwrap()
                .unwrap();
        assert_eq!(next.title, "for claude");

        let next = next_pickable_task(&store, &repo, "main", "p1", Some("other"), true, false)
            .unwrap()
            .unwrap();
        assert_eq!(next.title, "unassigned");

        let next = next_pickable_task(&store, &repo, "main", "p1", Some("other"), false, true)
            .unwrap();
        assert!(next.is_none());
    }

    #[test]
    fn next_pickable_task_respects_blockers() {
        let (repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let (t1, _) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "root",
            None,
            Some("medium"),
            None,
            None,
            vec![],
            "test-agent",
        )
        .unwrap();
        let (_t2, _) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "blocked",
            None,
            Some("critical"),
            None,
            None,
            vec![t1.id.as_str().to_string()],
            "test-agent",
        )
        .unwrap();
        let next = next_pickable_task(&store, &repo, "main", "p1", None, true, false)
            .unwrap()
            .unwrap();
        assert_eq!(next.title, "root");
    }

    #[test]
    fn task_to_json_includes_assigned_to() {
        let (repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        let (task, _) = add_task(
            &store,
            &repo,
            "main",
            "p1",
            "t",
            None,
            None,
            None,
            Some("claude-code"),
            vec![],
            "test-agent",
        )
        .unwrap();
        let assignments = read_assignments(&repo, "main", "p1");
        let value = task_to_json(&task, assignments.get(task.id.as_str()).map(|s| s.as_str()));
        assert_eq!(
            value.get("assigned_to").and_then(|v| v.as_str()),
            Some("claude-code")
        );
        assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("pending"));
    }

    #[test]
    fn list_plans_filter_by_status() {
        let (_repo, store) = fresh_store();
        create_plan(&store, "main", "p1", None).unwrap();
        create_plan(&store, "main", "p2", None).unwrap();
        store.archive_plan("main", "p2").unwrap();
        let plans = store.list_plans("main").unwrap();
        let active: Vec<_> = plans
            .iter()
            .filter(|p| p.status == PlanStatus::Active)
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "p1");
    }
}
