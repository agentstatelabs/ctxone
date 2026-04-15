"""CtxOne Hub HTTP client."""

from __future__ import annotations

import os
from typing import Any

import requests

from .exceptions import CtxOneError, HubUnreachable, MergeConflict, NotFound
from .types import (
    Commit,
    MemoryEntry,
    PrimeResult,
    RecallResult,
    RememberResult,
    Stats,
    TokenStats,
)


class Hub:
    """Client for the CtxOne Hub HTTP API.

    Example:
        >>> from ctxone import Hub
        >>> hub = Hub()  # defaults to http://localhost:3001
        >>> hub.remember("We use BSL-1.1", importance="high", context="legal")
        >>> result = hub.recall("licensing")
        >>> for r in result.results:
        ...     print(r.value)
        >>> print(f"{result.ctx_savings_ratio:.1f}x savings")
    """

    def __init__(
        self,
        server: str | None = None,
        *,
        branch: str = "main",
        timeout: float = 30.0,
        session: requests.Session | None = None,
        session_id: str | None = None,
    ) -> None:
        """Create a Hub client.

        Args:
            server: Hub URL. Defaults to the `CTX_SERVER` env var or
                `http://localhost:3001`.
            branch: Default branch/ref for reads and writes. Can be
                overridden per-call with the `ref` parameter.
            timeout: HTTP request timeout in seconds.
            session: Optional `requests.Session` for connection pooling
                and custom headers.
            session_id: Logical session identifier sent as the
                `X-CtxOne-Session` header on every request. The Hub
                accounts tokens-used per session so agents sharing a
                process can keep their stats separate. Defaults to the
                `CTX_SESSION_ID` env var, or `None` (Hub falls back to
                the `"default"` session).
        """
        self.server = (
            server
            or os.environ.get("CTX_SERVER")
            or "http://localhost:3001"
        ).rstrip("/")
        self.branch = branch
        self.timeout = timeout
        self._session = session or requests.Session()
        self.session_id = session_id or os.environ.get("CTX_SESSION_ID") or None

    # -- Health --------------------------------------------------------

    def is_reachable(self) -> bool:
        """Return True if the Hub responds to /api/health."""
        try:
            r = self._session.get(
                f"{self.server}/api/health", timeout=self.timeout
            )
            return r.ok
        except requests.RequestException:
            return False

    # -- Memory operations --------------------------------------------

    def remember(
        self,
        fact: str,
        *,
        importance: str = "medium",
        context: str | None = None,
        tags: list[str] | None = None,
        ref: str | None = None,
    ) -> RememberResult:
        """Store a fact in agent memory.

        Args:
            fact: The fact to remember.
            importance: "high", "medium", or "low". Maps to confidence 0.95/0.7/0.4.
            context: Category name (becomes `/memory/<context>/<id>`).
            tags: Queryable tags.
            ref: Branch to write to. Defaults to `self.branch`.

        Returns:
            RememberResult with the storage path and commit id.
        """
        body: dict[str, Any] = {
            "fact": fact,
            "importance": importance,
            "ref": ref or self.branch,
        }
        if context is not None:
            body["context"] = context
        if tags:
            body["tags"] = list(tags)

        data = self._post("/api/memory/remember", body)
        return RememberResult(
            path=data["path"],
            commit_id=data["commit_id"],
            ref=data.get("ref", self.branch),
        )

    def forget(
        self,
        path: str,
        *,
        reason: str | None = None,
        ref: str | None = None,
    ) -> str:
        """Delete a memory at a specific path.

        Returns the commit id of the rollback commit.
        """
        body: dict[str, Any] = {"path": path, "ref": ref or self.branch}
        if reason is not None:
            body["reason"] = reason

        data = self._post("/api/memory/forget", body)
        return data["commit_id"]

    def recall(
        self,
        topic: str,
        *,
        budget: int = 1500,
        ref: str | None = None,
    ) -> RecallResult:
        """Retrieve memories relevant to a topic.

        Always includes pinned memories first, then topic-matched facts,
        respecting a token budget.
        """
        params = {
            "topic": topic,
            "budget": str(budget),
            "ref": ref or self.branch,
        }
        data = self._get("/api/memory/recall", params=params)

        entries: list[MemoryEntry] = []
        for r in data.get("results", []):
            entries.append(
                MemoryEntry(
                    path=r.get("path", ""),
                    pinned=r.get("pinned", False),
                    value=r.get("value"),
                    title=r.get("title"),
                    body=r.get("body"),
                    score=r.get("score"),
                    full_match=r.get("full_match"),
                )
            )

        return RecallResult(
            topic=topic,
            results=entries,
            pinned_count=data.get("pinned_count", 0),
            topic_matches=data.get("topic_matches", 0),
            ctx_tokens_sent=data.get("ctx_tokens_sent", 0),
            ctx_tokens_estimated_flat=data.get("ctx_tokens_estimated_flat", 0),
            ctx_savings_ratio=data.get("ctx_savings_ratio", 0.0),
            ref=data.get("ref", ref or self.branch),
            raw=data,
        )

    def prime(
        self,
        source: str,
        sections: list[dict[str, str]],
        *,
        pinned: bool = False,
        ref: str | None = None,
    ) -> PrimeResult:
        """Load a list of {title, body} sections as primed or pinned memory.

        Args:
            source: Group name. Re-calling prime() with the same source
                overwrites (idempotent).
            sections: List of dicts with `title` and `body` keys.
            pinned: If True, these memories are always included in recall.
            ref: Branch to write to.
        """
        body = {
            "source": source,
            "pinned": pinned,
            "sections": sections,
            "ref": ref or self.branch,
        }
        data = self._post("/api/memory/prime", body)
        return PrimeResult(
            source=source,
            pinned=pinned,
            sections_written=data.get("sections_written", 0),
            paths=data.get("paths", []),
            ref=data.get("ref", ref or self.branch),
        )

    def prime_markdown(
        self,
        source: str,
        markdown: str,
        *,
        pinned: bool = False,
        ref: str | None = None,
    ) -> PrimeResult:
        """Parse a markdown string at H1/H2 headings and prime the sections.

        Convenience wrapper around prime() that handles parsing client-side.
        """
        sections = _parse_markdown_sections(markdown)
        if not sections:
            raise CtxOneError(
                "No sections found in markdown (need H1 or H2 headings)"
            )
        return self.prime(source, sections, pinned=pinned, ref=ref)

    def pinned(self) -> list[dict[str, Any]]:
        """Return all pinned memories as raw {path, value} pairs.

        Typically you'll want to group these by source and pair
        /title and /body children. See pinned_grouped() for that.
        """
        return self._get("/api/memory/pinned")

    def pinned_grouped(self) -> dict[str, list[dict[str, str]]]:
        """Return pinned memories grouped by source.

        Returns:
            A dict mapping source name to a list of `{title, body}` sections.
        """
        items = self.pinned()
        grouped: dict[str, dict[str, dict[str, str]]] = {}
        for item in items:
            path = item.get("path", "")
            value = item.get("value")
            # path: /memory/pinned/<source>/<slug>/(title|body)
            parts = path.split("/")
            if len(parts) < 6:
                continue
            _, _, _, source, slug, field = parts[:6]
            if source not in grouped:
                grouped[source] = {}
            if slug not in grouped[source]:
                grouped[source][slug] = {}
            if field in ("title", "body") and isinstance(value, str):
                grouped[source][slug][field] = value

        return {
            source: [
                section
                for section in sections.values()
                if "title" in section and "body" in section
            ]
            for source, sections in grouped.items()
        }

    def context(self, project: str, *, ref: str | None = None) -> Any:
        """Load the full context subtree for a project."""
        data = self._get(
            f"/api/memory/context/{project}",
            params={"ref": ref or self.branch},
        )
        return data.get("context")

    # -- Graph visibility ---------------------------------------------

    def search(
        self,
        query: str,
        *,
        max_results: int = 50,
        ref: str | None = None,
    ) -> list[dict[str, str]]:
        """Literal substring search across values and keys.

        Unlike recall, this returns all matches without a token budget.
        """
        params = {"query": query, "max_results": str(max_results)}
        ref = ref or self.branch
        return self._get(f"/api/state/{ref}/search", params=params)

    def ls(
        self,
        prefix: str = "/",
        *,
        max_depth: int = 50,
        ref: str | None = None,
    ) -> list[str]:
        """List all paths under a prefix."""
        params = {"prefix": prefix, "max_depth": str(max_depth)}
        ref = ref or self.branch
        return self._get(f"/api/state/{ref}/paths", params=params)

    def get(self, path: str, *, ref: str | None = None) -> Any:
        """Read the value at a specific path."""
        ref = ref or self.branch
        return self._get(f"/api/state/{ref}", params={"path": path})

    def log(self, *, limit: int = 20, ref: str | None = None) -> list[Commit]:
        """Recent commit history."""
        ref = ref or self.branch
        data = self._get(f"/api/log/{ref}", params={"limit": str(limit)})
        commits: list[Commit] = []
        for c in data:
            intent = c.get("intent", {})
            commits.append(
                Commit(
                    id=c.get("id", ""),
                    timestamp=c.get("timestamp", ""),
                    agent_id=c.get("agent_id", ""),
                    description=intent.get("description", ""),
                    category=intent.get("category", ""),
                    confidence=c.get("confidence"),
                    reasoning=c.get("reasoning"),
                    tags=intent.get("tags", []),
                )
            )
        return commits

    def blame(self, path: str, *, ref: str | None = None) -> Any:
        """Return the provenance chain for a path."""
        ref = ref or self.branch
        return self._get(f"/api/blame/{ref}", params={"path": path})

    def diff(self, ref_a: str, ref_b: str) -> list[dict[str, Any]]:
        """Diff two refs. Returns a list of DiffOp dicts."""
        data = self._get("/api/diff", params={"ref_a": ref_a, "ref_b": ref_b})
        return data.get("ops", [])

    # -- Branches ------------------------------------------------------

    def branches(self) -> list[dict[str, str]]:
        """List all branches."""
        return self._get("/api/branches")

    def create_branch(self, name: str, *, from_: str = "main") -> str:
        """Create a new branch. Returns the commit id the branch starts at.

        Note: named `create_branch` (not `branch`) to avoid colliding with
        the `self.branch` attribute that holds the default branch for reads.
        """
        data = self._post("/api/branches", {"name": name, "from": from_})
        return data["commit_id"]

    def merge(
        self,
        source: str,
        *,
        into: str = "main",
        description: str | None = None,
    ) -> str:
        """Merge a source branch into a target.

        Raises MergeConflict if the engine can't auto-resolve.
        """
        body: dict[str, Any] = {"source": source, "target": into}
        if description is not None:
            body["description"] = description

        try:
            data = self._post("/api/merge", body)
            return data["commit_id"]
        except CtxOneError as e:
            # The POST helper already raises MergeConflict on 409;
            # if we got here with a different CtxOneError, re-raise.
            raise e

    # -- Stats ---------------------------------------------------------

    def stats(self) -> TokenStats:
        """Cumulative token savings for the current Hub session."""
        data = self._get("/api/stats/tokens")
        return TokenStats(
            session_tokens_used=data.get("session_tokens_used", 0),
            session_tokens_saved=data.get("session_tokens_saved", 0),
            total_graph_size_chars=data.get("total_graph_size_chars", 0),
            total_graph_size_tokens=data.get("total_graph_size_tokens", 0),
            cumulative_ratio=data.get("cumulative_ratio", 0.0),
        )

    def branch_stats(self, ref: str | None = None) -> Stats:
        """Structural stats for a branch."""
        ref = ref or self.branch
        data = self._get(f"/api/stats/{ref}")
        return Stats(
            commit_count=data.get("commit_count", 0),
            path_count=data.get("path_count", 0),
            branch_count=data.get("branch_count", 0),
            epoch_count=data.get("epoch_count", 0),
            agents=data.get("agents", []),
            categories=data.get("categories", []),
            raw=data,
        )

    # -- HTTP helpers (private) ---------------------------------------

    def _headers(self) -> dict[str, str]:
        """Build the request header dict with X-CtxOne-Session attached."""
        headers: dict[str, str] = {}
        if self.session_id:
            headers["X-CtxOne-Session"] = self.session_id
        return headers

    def _get(self, path: str, params: dict[str, str] | None = None) -> Any:
        url = f"{self.server}{path}"
        try:
            r = self._session.get(
                url,
                params=params,
                headers=self._headers(),
                timeout=self.timeout,
            )
        except requests.ConnectionError as e:
            raise HubUnreachable(f"Hub unreachable at {self.server}: {e}") from e
        except requests.RequestException as e:
            raise CtxOneError(f"request failed: {e}") from e
        return self._parse_response(r)

    def _post(self, path: str, body: Any) -> Any:
        url = f"{self.server}{path}"
        try:
            r = self._session.post(
                url,
                json=body,
                headers=self._headers(),
                timeout=self.timeout,
            )
        except requests.ConnectionError as e:
            raise HubUnreachable(f"Hub unreachable at {self.server}: {e}") from e
        except requests.RequestException as e:
            raise CtxOneError(f"request failed: {e}") from e
        return self._parse_response(r)

    def _parse_response(self, r: requests.Response) -> Any:
        if r.status_code == 404:
            raise NotFound(f"{r.url}: {r.text}")
        if r.status_code == 409:
            # Merge conflict carries a JSON body with conflicts
            try:
                data = r.json()
                raise MergeConflict(
                    "merge conflict",
                    conflicts=data.get("conflicts", []),
                )
            except (ValueError, KeyError):
                raise MergeConflict(f"merge conflict: {r.text}")
        if r.status_code >= 500:
            raise CtxOneError(f"server error {r.status_code}: {r.text}")
        if not r.ok:
            raise CtxOneError(f"{r.status_code}: {r.text}")

        if r.headers.get("content-type", "").startswith("application/json"):
            return r.json()
        return r.text


def _parse_markdown_sections(content: str) -> list[dict[str, str]]:
    """Parse markdown into {title, body} sections at H1/H2 headings.

    Mirrors the CLI's parse_markdown_sections() for feature parity.
    """
    sections: list[dict[str, str]] = []
    current_title: str | None = None
    current_body: list[str] = []

    def flush() -> None:
        body = "\n".join(current_body).strip()
        if not body:
            return
        sections.append(
            {
                "title": current_title or "Intro",
                "body": body,
            }
        )

    for line in content.splitlines():
        stripped = line.lstrip()
        is_h1 = stripped.startswith("# ") and not stripped.startswith("## ")
        is_h2 = stripped.startswith("## ") and not stripped.startswith("### ")

        if is_h1 or is_h2:
            flush()
            current_body = []
            prefix_len = 2 if is_h1 else 3
            current_title = stripped[prefix_len:].strip()
        else:
            current_body.append(line)

    flush()
    return sections
