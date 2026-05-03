//! CTXone reminder tools — wrap `agentstategraph-reminders` and surface
//! it as MCP tools + shared helpers for the HTTP endpoints.
//!
//! Reminders are future-oriented, pull-based scheduling primitives.
//! Agents create reminders with a due time and instructions, then call
//! `remind_me` at natural checkpoints (session start, task completion,
//! branch switch) to retrieve actionable items ordered by priority.
//!
//! The tool methods themselves live on `CtxOneServer` in `memory_tools.rs`
//! so every MCP tool shares a single `ToolRouter`; this module provides
//! the typed parameter structs and the response-shape helpers.

use std::sync::Arc;

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use agentstategraph_reminders::{
    CreateReminder, ExecutionRecord, ExecutionResult, Priority, RefKind, Reminder, ReminderError,
    ReminderFilter, ReminderManager, ReminderRef, ReminderStatus, ReminderStore, Schedule,
};

// ---------------------------------------------------------------------------
// Manager constructor
// ---------------------------------------------------------------------------

/// Build a `ReminderManager` from any `Arc<dyn ReminderStore>`.
/// Pass `repo.clone()` — `Repository` implements `ReminderStore` by
/// delegating to its underlying storage, so reminders go to the same
/// SQLite / Postgres / memory backend as everything else.
pub fn make_manager(store: Arc<dyn ReminderStore>) -> ReminderManager {
    ReminderManager::new(store)
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ReminderToolError {
    #[error("{0}")]
    Store(#[from] ReminderError),

    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub fn err_json(e: impl std::fmt::Display) -> String {
    serde_json::to_string(&serde_json::json!({ "error": e.to_string() }))
        .unwrap_or_else(|_| "{\"error\":\"unknown\"}".to_string())
}

// ---------------------------------------------------------------------------
// Response shape helpers
// ---------------------------------------------------------------------------

pub fn priority_label(p: Priority) -> &'static str {
    match p {
        Priority::Critical => "critical",
        Priority::High => "high",
        Priority::Medium => "medium",
        Priority::Low => "low",
        Priority::Minimal => "minimal",
    }
}

pub fn status_label(s: ReminderStatus) -> &'static str {
    match s {
        ReminderStatus::Pending => "pending",
        ReminderStatus::Due => "due",
        ReminderStatus::AwaitingPermission => "awaiting_permission",
        ReminderStatus::InProgress => "in_progress",
        ReminderStatus::Completed => "completed",
        ReminderStatus::Snoozed => "snoozed",
        ReminderStatus::Cancelled => "cancelled",
    }
}

pub fn result_label(r: &ExecutionResult) -> &'static str {
    match r {
        ExecutionResult::Success => "success",
        ExecutionResult::Failed => "failed",
        ExecutionResult::Deferred => "deferred",
        ExecutionResult::Snoozed => "snoozed",
        ExecutionResult::Cancelled => "cancelled",
    }
}

/// Render a `Reminder` as the JSON shape CTXone exposes on the wire.
pub fn reminder_to_json(r: &Reminder) -> serde_json::Value {
    let schedule = r.schedule.as_ref().map(|s| match s {
        Schedule::Once => serde_json::json!({ "kind": "once" }),
        Schedule::Interval { every_seconds } => {
            serde_json::json!({ "kind": "interval", "every_seconds": every_seconds })
        }
        Schedule::Daily { time } => {
            serde_json::json!({ "kind": "daily", "time": time.format("%H:%M").to_string() })
        }
        Schedule::Weekly { day, time } => {
            serde_json::json!({
                "kind": "weekly",
                "day": format!("{:?}", day).to_lowercase(),
                "time": time.format("%H:%M").to_string()
            })
        }
    });

    let refs: Vec<serde_json::Value> = r
        .refs
        .iter()
        .map(|rf| {
            serde_json::json!({
                "kind": match &rf.kind {
                    RefKind::Branch => "branch",
                    RefKind::Memory => "memory",
                    RefKind::Plan => "plan",
                    RefKind::Task => "task",
                    RefKind::StatePath => "state_path",
                    RefKind::External { .. } => "external",
                },
                "id": rf.id,
                "label": rf.label,
                "stale": rf.stale,
            })
        })
        .collect();

    let executions: Vec<serde_json::Value> = r
        .executions
        .iter()
        .map(|ex| {
            serde_json::json!({
                "started_at": ex.started_at.to_rfc3339(),
                "completed_at": ex.completed_at.map(|t| t.to_rfc3339()),
                "agent_id": ex.agent_id,
                "approved_by": ex.approved_by,
                "result": result_label(&ex.result),
                "notes": ex.notes,
                "task_id": ex.task_id,
            })
        })
        .collect();

    serde_json::json!({
        "id": r.id,
        "title": r.title,
        "instructions": r.instructions,
        "commands": r.commands,
        "refs": refs,
        "priority": priority_label(r.priority),
        "due_at": r.due_at.to_rfc3339(),
        "schedule": schedule,
        "autonomous": r.autonomous,
        "created_by": r.created_by,
        "created_at": r.created_at.to_rfc3339(),
        "status": status_label(r.status),
        "snoozed_until": r.snoozed_until.map(|t| t.to_rfc3339()),
        "executions": executions,
        "tags": r.tags,
    })
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

pub fn parse_priority(s: &str) -> Option<Priority> {
    match s.to_ascii_lowercase().as_str() {
        "critical" => Some(Priority::Critical),
        "high" => Some(Priority::High),
        "medium" => Some(Priority::Medium),
        "low" => Some(Priority::Low),
        "minimal" => Some(Priority::Minimal),
        _ => None,
    }
}

pub fn parse_status(s: &str) -> Option<ReminderStatus> {
    match s.to_ascii_lowercase().as_str() {
        "pending" => Some(ReminderStatus::Pending),
        "due" => Some(ReminderStatus::Due),
        "awaiting_permission" => Some(ReminderStatus::AwaitingPermission),
        "in_progress" => Some(ReminderStatus::InProgress),
        "completed" => Some(ReminderStatus::Completed),
        "snoozed" => Some(ReminderStatus::Snoozed),
        "cancelled" => Some(ReminderStatus::Cancelled),
        _ => None,
    }
}

pub fn parse_result(s: &str) -> Option<ExecutionResult> {
    match s.to_ascii_lowercase().as_str() {
        "success" => Some(ExecutionResult::Success),
        "failed" => Some(ExecutionResult::Failed),
        "deferred" => Some(ExecutionResult::Deferred),
        "snoozed" => Some(ExecutionResult::Snoozed),
        "cancelled" => Some(ExecutionResult::Cancelled),
        _ => None,
    }
}

/// Parse an ISO 8601 / RFC 3339 datetime string.
pub fn parse_datetime(s: &str) -> Result<DateTime<Utc>, ReminderToolError> {
    s.parse::<DateTime<Utc>>().map_err(|_| {
        ReminderToolError::InvalidInput(format!(
            "invalid datetime '{}' — use ISO 8601 / RFC 3339 (e.g. 2026-05-02T14:00:00Z)",
            s
        ))
    })
}

/// Parse an `HH:MM` time string for Daily/Weekly schedules.
fn parse_hhmm(s: &str) -> Result<NaiveTime, ReminderToolError> {
    NaiveTime::parse_from_str(s, "%H:%M").map_err(|_| {
        ReminderToolError::InvalidInput(format!(
            "invalid time '{}' — use HH:MM (e.g. 09:00)",
            s
        ))
    })
}

/// Parse a weekday name (Mon/Monday/mon…).
fn parse_weekday(s: &str) -> Option<Weekday> {
    match s.to_ascii_lowercase().trim_end_matches("day") {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// MCP / HTTP parameter types
// ---------------------------------------------------------------------------

/// Recurrence schedule for `reminder_create`.
///
/// `kind` must be one of: `once` | `interval` | `daily` | `weekly`.
#[derive(Deserialize, JsonSchema)]
pub struct ScheduleParam {
    /// once | interval | daily | weekly
    pub kind: String,
    /// Seconds between executions (required for `interval`).
    pub every_seconds: Option<u64>,
    /// UTC wall-clock time as `HH:MM` (required for `daily` and `weekly`).
    pub time: Option<String>,
    /// Day of week: Mon/Tue/Wed/Thu/Fri/Sat/Sun (required for `weekly`).
    pub day: Option<String>,
}

impl ScheduleParam {
    pub fn into_schedule(self) -> Result<Schedule, ReminderToolError> {
        match self.kind.to_ascii_lowercase().as_str() {
            "once" => Ok(Schedule::Once),
            "interval" => {
                let secs = self.every_seconds.ok_or_else(|| {
                    ReminderToolError::InvalidInput(
                        "interval schedule requires every_seconds".into(),
                    )
                })?;
                Ok(Schedule::Interval { every_seconds: secs })
            }
            "daily" => {
                let t = self.time.ok_or_else(|| {
                    ReminderToolError::InvalidInput("daily schedule requires time (HH:MM)".into())
                })?;
                Ok(Schedule::Daily { time: parse_hhmm(&t)? })
            }
            "weekly" => {
                let t = self.time.ok_or_else(|| {
                    ReminderToolError::InvalidInput("weekly schedule requires time (HH:MM)".into())
                })?;
                let d_str = self.day.ok_or_else(|| {
                    ReminderToolError::InvalidInput(
                        "weekly schedule requires day (Mon/Tue/…)".into(),
                    )
                })?;
                let day = parse_weekday(&d_str).ok_or_else(|| {
                    ReminderToolError::InvalidInput(format!(
                        "unknown weekday '{}' — use Mon/Tue/Wed/Thu/Fri/Sat/Sun",
                        d_str
                    ))
                })?;
                Ok(Schedule::Weekly { day, time: parse_hhmm(&t)? })
            }
            other => Err(ReminderToolError::InvalidInput(format!(
                "unknown schedule kind '{}' — use once|interval|daily|weekly",
                other
            ))),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderCreateParams {
    /// Short title (imperative, one line).
    pub title: String,
    /// Full instructions the agent should follow at execution time.
    pub instructions: String,
    /// Optional specific tool calls / commands to run.
    #[serde(default)]
    pub commands: Vec<String>,
    /// ISO 8601 / RFC 3339 datetime when this reminder becomes due
    /// (e.g. `2026-05-05T09:00:00Z`).
    pub due_at: String,
    /// Priority: critical | high | medium (default) | low | minimal.
    pub priority: Option<String>,
    /// `true` (default): agent may execute without user approval.
    /// `false`: surfaces as `awaiting_permission` until `reminder_approve` is called.
    pub autonomous: Option<bool>,
    /// Recurrence schedule. Omit for a one-shot reminder.
    pub schedule: Option<ScheduleParam>,
    /// Soft references: list of `{kind, id, label?}` objects.
    /// `kind` is one of: branch | memory | plan | task | state_path | external.
    #[serde(default)]
    pub refs: Vec<ReminderRefParam>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A soft reference to another object, for `ReminderCreateParams.refs`.
#[derive(Deserialize, JsonSchema)]
pub struct ReminderRefParam {
    /// branch | memory | plan | task | state_path | external
    pub kind: String,
    pub id: String,
    pub label: Option<String>,
    /// Required when `kind = "external"` (e.g. `"https"`, `"file"`).
    pub scheme: Option<String>,
}

impl ReminderRefParam {
    pub fn into_reminder_ref(self) -> Result<ReminderRef, ReminderToolError> {
        let kind = match self.kind.to_ascii_lowercase().as_str() {
            "branch" => RefKind::Branch,
            "memory" => RefKind::Memory,
            "plan" => RefKind::Plan,
            "task" => RefKind::Task,
            "state_path" => RefKind::StatePath,
            "external" => {
                let scheme = self.scheme.unwrap_or_else(|| "https".into());
                RefKind::External { scheme }
            }
            other => {
                return Err(ReminderToolError::InvalidInput(format!(
                    "unknown ref kind '{}' — use branch|memory|plan|task|state_path|external",
                    other
                )));
            }
        };
        Ok(ReminderRef {
            kind,
            id: self.id,
            label: self.label,
            stale: false,
        })
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderListParams {
    /// Filter by status: pending | due | awaiting_permission | in_progress | completed | snoozed | cancelled.
    pub status: Option<String>,
    /// Return reminders with priority <= this value (critical > high > medium > low > minimal).
    pub priority_at_most: Option<String>,
    /// Filter by creator (agent id or user).
    pub created_by: Option<String>,
    /// Return only reminders due at or before this ISO 8601 datetime.
    pub due_before: Option<String>,
    /// Return only reminders that have a soft ref with this id.
    pub ref_id: Option<String>,
    /// All listed tags must be present on the reminder.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// `remind_me` has no parameters — it returns all actionable (Due /
/// AwaitingPermission) reminders, lazily promoting any Pending reminders
/// whose `due_at` has passed.
#[derive(Deserialize, JsonSchema)]
pub struct RemindMeParams {}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderGetParams {
    /// Reminder id.
    pub id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderSnoozeParams {
    /// Reminder id.
    pub id: String,
    /// Wake-up time as ISO 8601 / RFC 3339 (e.g. `2026-05-03T08:00:00Z`).
    pub until: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderApproveParams {
    /// Reminder id.
    pub id: String,
    /// Who is approving (agent id or user). Defaults to server agent id.
    pub approved_by: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderCancelParams {
    /// Reminder id.
    pub id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderStartParams {
    /// Reminder id.
    pub id: String,
    /// Agent id performing the execution. Defaults to server agent id.
    pub agent_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReminderRecordParams {
    /// Reminder id.
    pub id: String,
    /// Outcome: success | failed | deferred | snoozed | cancelled.
    pub result: String,
    /// Free-form notes about this execution attempt.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Task id created for this execution (if any).
    pub task_id: Option<String>,
    /// Agent id performing the execution. Defaults to server agent id.
    pub agent_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Business-logic helpers called by both MCP tools and HTTP endpoints
// ---------------------------------------------------------------------------

pub fn create_reminder(
    mgr: &ReminderManager,
    params: ReminderCreateParams,
    default_agent: &str,
) -> Result<Reminder, ReminderToolError> {
    let due_at = parse_datetime(&params.due_at)?;
    let priority = match params.priority.as_deref() {
        None => Priority::Medium,
        Some(s) => parse_priority(s).ok_or_else(|| {
            ReminderToolError::InvalidInput(format!(
                "unknown priority '{}' — use critical|high|medium|low|minimal",
                s
            ))
        })?,
    };

    let refs: Vec<ReminderRef> = params
        .refs
        .into_iter()
        .map(|r| r.into_reminder_ref())
        .collect::<Result<_, _>>()?;

    let schedule = params.schedule.map(|s| s.into_schedule()).transpose()?;

    let mut cr = CreateReminder::new(
        params.title,
        params.instructions,
        due_at,
        default_agent.to_string(),
    )
    .with_priority(priority)
    .with_autonomous(params.autonomous.unwrap_or(true))
    .with_commands(params.commands)
    .with_refs(refs)
    .with_tags(params.tags);

    if let Some(s) = schedule {
        cr = cr.with_schedule(s);
    }

    Ok(mgr.create(cr)?)
}

pub fn list_reminders(
    mgr: &ReminderManager,
    params: ReminderListParams,
) -> Result<Vec<Reminder>, ReminderToolError> {
    let status = params
        .status
        .as_deref()
        .map(|s| {
            parse_status(s).ok_or_else(|| {
                ReminderToolError::InvalidInput(format!(
                    "unknown status '{}' — use pending|due|awaiting_permission|in_progress|completed|snoozed|cancelled",
                    s
                ))
            })
        })
        .transpose()?;

    let priority_at_most = params
        .priority_at_most
        .as_deref()
        .map(|s| {
            parse_priority(s).ok_or_else(|| {
                ReminderToolError::InvalidInput(format!("unknown priority '{}'", s))
            })
        })
        .transpose()?;

    let due_before = params
        .due_before
        .as_deref()
        .map(parse_datetime)
        .transpose()?;

    let filter = ReminderFilter {
        status,
        priority_at_most,
        created_by: params.created_by,
        due_before,
        ref_id: params.ref_id,
        tags: params.tags,
    };

    Ok(mgr.list(&filter)?)
}

pub fn record_execution(
    mgr: &ReminderManager,
    params: ReminderRecordParams,
    default_agent: &str,
) -> Result<Reminder, ReminderToolError> {
    let result = parse_result(&params.result).ok_or_else(|| {
        ReminderToolError::InvalidInput(format!(
            "unknown result '{}' — use success|failed|deferred|snoozed|cancelled",
            params.result
        ))
    })?;

    let record = ExecutionRecord {
        started_at: Utc::now(),
        completed_at: None,
        agent_id: params
            .agent_id
            .unwrap_or_else(|| default_agent.to_string()),
        approved_by: None,
        result,
        notes: params.notes,
        task_id: params.task_id,
    };

    Ok(mgr.record_execution(&params.id, record)?)
}

// ---------------------------------------------------------------------------
// Serialize helpers used by HTTP endpoints
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ToolErr {
    pub error: String,
}
