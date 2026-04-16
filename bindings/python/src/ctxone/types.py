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
