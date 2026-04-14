"""Exception types for the CtxOne client."""

from __future__ import annotations


class CtxOneError(Exception):
    """Base exception for all CtxOne client errors."""


class HubUnreachable(CtxOneError):
    """Raised when the Hub server cannot be reached (network error, wrong URL)."""


class NotFound(CtxOneError):
    """Raised when a resource (path, branch, commit) is not found."""


class MergeConflict(CtxOneError):
    """Raised when a merge operation hits unresolvable conflicts.

    The `.conflicts` attribute contains the raw conflict list from the Hub.
    """

    def __init__(self, message: str, conflicts: list | None = None) -> None:
        super().__init__(message)
        self.conflicts = conflicts or []
