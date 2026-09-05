"""
CtxOne — persistent, searchable, accountable memory for AI agents.

Quick start:

    from ctxone import Hub

    hub = Hub("http://localhost:3001")
    hub.remember("BSL-1.1 licensing", importance="high", context="legal")

    result = hub.recall("licensing")
    for r in result.results:
        print(r.path, "-", r.value)
    print(f"{result.ctx_savings_ratio:.1f}x savings")

See https://github.com/AgentStateLabs/CTXone for docs.
"""

from .client import Hub
from .exceptions import CtxOneError, HubUnreachable, MergeConflict, NotFound
from .types import (
    Commit,
    MemoryEntry,
    Plan,
    PlanStatus,
    PrimeResult,
    Priority,
    Proof,
    ProofKind,
    RecallResult,
    RememberResult,
    SessionSnapshot,
    Stats,
    Task,
    TaskCounts,
    TaskStatus,
    TokenStats,
)

__version__ = "0.74.0"

__all__ = [
    "Hub",
    "CtxOneError",
    "HubUnreachable",
    "MergeConflict",
    "NotFound",
    "Commit",
    "MemoryEntry",
    "Plan",
    "PlanStatus",
    "PrimeResult",
    "Priority",
    "Proof",
    "ProofKind",
    "RecallResult",
    "RememberResult",
    "SessionSnapshot",
    "Stats",
    "Task",
    "TaskCounts",
    "TaskStatus",
    "TokenStats",
    "__version__",
]
