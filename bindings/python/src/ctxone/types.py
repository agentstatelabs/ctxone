"""Typed return values for the CtxOne client.

All types are lightweight dataclasses. The `.raw` field on complex types
preserves the original JSON response in case you need a field this binding
doesn't expose yet.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass
class RememberResult:
    """Result of a `remember()` call — where the fact landed."""

    path: str
    commit_id: str
    ref: str = "main"


@dataclass
class MemoryEntry:
    """One entry from a `recall()` result.

    Pinned entries have `title` and `body` populated (they came from
    `prime()`). Topic-matched entries have `value` instead.
    """

    path: str
    pinned: bool
    value: str | None = None
    title: str | None = None
    body: str | None = None
    score: int | None = None
    full_match: bool | None = None


@dataclass
class RecallResult:
    """Result of a `recall()` call, including token savings metadata."""

    topic: str
    results: list[MemoryEntry]
    pinned_count: int
    topic_matches: int
    ctx_tokens_sent: int
    ctx_tokens_estimated_flat: int
    ctx_savings_ratio: float
    ref: str = "main"
    raw: dict[str, Any] = field(default_factory=dict)


@dataclass
class PrimeResult:
    """Result of a `prime()` call."""

    source: str
    pinned: bool
    sections_written: int
    paths: list[str]
    ref: str = "main"


@dataclass
class Commit:
    """A commit entry from `log()` or `blame()`."""

    id: str
    timestamp: str
    agent_id: str
    description: str
    category: str
    confidence: float | None = None
    reasoning: str | None = None
    tags: list[str] = field(default_factory=list)


@dataclass
class TokenStats:
    """Hub-session-wide token savings totals from `stats()`."""

    session_tokens_used: int
    session_tokens_saved: int
    total_graph_size_chars: int
    total_graph_size_tokens: int
    cumulative_ratio: float


@dataclass
class Stats:
    """Structural stats for a branch from `branch_stats()`."""

    commit_count: int
    path_count: int
    branch_count: int
    epoch_count: int
    agents: list[str]
    categories: list[str]
    raw: dict[str, Any] = field(default_factory=dict)


@dataclass
class Proof:
    """Evidence attached to a `done` task."""

    kind: str  # commit | file | test | text
    value: str
    note: str | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"kind": self.kind, "value": self.value}
        if self.note is not None:
            d["note"] = self.note
        return d


@dataclass
class ProofKind:
    """String constants for proof kinds. Convenience for callers who
    don't want to memorise the strings."""

    COMMIT: str = "commit"
    FILE: str = "file"
    TEST: str = "test"
    TEXT: str = "text"


@dataclass
class TaskStatus:
    """String constants for task status values."""

    PENDING: str = "pending"
    IN_PROGRESS: str = "in_progress"
    DONE: str = "done"
    ABANDONED: str = "abandoned"


@dataclass
class PlanStatus:
    """String constants for plan status values."""

    ACTIVE: str = "active"
    COMPLETED: str = "completed"
    ARCHIVED: str = "archived"


@dataclass
class Priority:
    """String constants for task priority values."""

    LOW: str = "low"
    MEDIUM: str = "medium"
    HIGH: str = "high"
    CRITICAL: str = "critical"


@dataclass
class Task:
    """A unit of work inside a plan, mirroring the Hub's JSON shape."""

    id: str
    title: str
    status: str
    priority: str
    parent_id: str | None = None
    blocked_by: list[str] = field(default_factory=list)
    assigned_to: str | None = None
    created_at: str | None = None
    created_by: str | None = None
    started_at: str | None = None
    started_by: str | None = None
    completed_at: str | None = None
    completed_by: str | None = None
    abandoned_at: str | None = None
    abandoned_reason: str | None = None
    proof: Proof | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Task":
        proof_data = data.get("proof")
        proof = (
            Proof(
                kind=proof_data.get("kind", ""),
                value=proof_data.get("value", ""),
                note=proof_data.get("note"),
            )
            if isinstance(proof_data, dict)
            else None
        )
        return cls(
            id=data.get("id", ""),
            title=data.get("title", ""),
            status=data.get("status", ""),
            priority=data.get("priority", "medium"),
            parent_id=data.get("parent_id"),
            blocked_by=list(data.get("blocked_by", []) or []),
            assigned_to=data.get("assigned_to"),
            created_at=data.get("created_at"),
            created_by=data.get("created_by"),
            started_at=data.get("started_at"),
            started_by=data.get("started_by"),
            completed_at=data.get("completed_at"),
            completed_by=data.get("completed_by"),
            abandoned_at=data.get("abandoned_at"),
            abandoned_reason=data.get("abandoned_reason"),
            proof=proof,
            raw=data,
        )


@dataclass
class TaskCounts:
    """Counts of tasks keyed by status."""

    pending: int = 0
    in_progress: int = 0
    done: int = 0
    abandoned: int = 0
    total: int = 0

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "TaskCounts":
        return cls(
            pending=int(data.get("pending", 0)),
            in_progress=int(data.get("in_progress", 0)),
            done=int(data.get("done", 0)),
            abandoned=int(data.get("abandoned", 0)),
            total=int(data.get("total", 0)),
        )


@dataclass
class Plan:
    """A plan — named container of tasks."""

    name: str
    status: str
    description: str | None = None
    created_at: str | None = None
    created_by: str | None = None
    archived_at: str | None = None
    task_counts: TaskCounts = field(default_factory=TaskCounts)
    tasks: list[Task] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Plan":
        counts = TaskCounts.from_dict(data.get("task_counts", {}))
        tasks = [Task.from_dict(t) for t in data.get("tasks", []) or []]
        return cls(
            name=data.get("name", ""),
            status=data.get("status", "active"),
            description=data.get("description"),
            created_at=data.get("created_at"),
            created_by=data.get("created_by"),
            archived_at=data.get("archived_at"),
            task_counts=counts,
            tasks=tasks,
            raw=data,
        )


@dataclass
class SessionSnapshot:
    """A snapshot of one session's cumulative counters.

    Returned by `Hub.record_usage()` so callers can see the running
    totals in one round trip. Also the shape of
    `GET /api/stats/tokens/{session_id}` responses.

    The `llm_*` fields and `last_model`/`last_provider` are the
    LLM-observed counters populated by `record_usage` — they stay at
    `0` / `None` until an agent reports usage for the session.
    """

    session_id: str
    session_tokens_used: int = 0
    session_tokens_saved: int = 0
    total_graph_size_chars: int = 0
    total_graph_size_tokens: int = 0
    cumulative_ratio: float = 0.0
    llm_input_tokens: int = 0
    llm_output_tokens: int = 0
    llm_cache_read_tokens: int = 0
    llm_cache_create_tokens: int = 0
    llm_call_count: int = 0
    last_model: str | None = None
    last_provider: str | None = None
