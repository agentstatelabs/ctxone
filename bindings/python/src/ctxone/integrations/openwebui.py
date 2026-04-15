"""
title: CtxOne Memory
author: Craig Brown
author_url: https://github.com/ctxone
git_url: https://github.com/ctxone/ctxone
description: Persistent, searchable, accountable memory for Open WebUI chats — via CtxOne Hub.
required_open_webui_version: 0.4.0
requirements: ctxone>=0.73.0
version: 0.73.0
license: BSL-1.1

This module provides two Open WebUI plugins that share a Hub client:

    - `Tools`   — function-calling tools the model can invoke explicitly.
                  Exposes `remember`, `recall`, `forget`, and `list_pinned`.

    - `Filter`  — in-process inlet/outlet/stream hooks that run around
                  every model call without tool-calling. The inlet
                  auto-injects relevant memories into the system prompt
                  based on the user's most recent message. The outlet
                  (optionally) captures the assistant's reply as a fact.

You can use either or both. The Filter is the interesting one — it makes
CtxOne behave like background memory the model doesn't have to ask for,
which also means it works with models that don't support tool-calling.

## Install as an Open WebUI function

Copy the contents of this file into Open WebUI → Admin Panel → Functions →
**+** → paste. The frontmatter docstring at the top lets Open WebUI
auto-pip-install `ctxone>=0.73.0` on first load (requires
`ENABLE_PIP_INSTALL_FRONTMATTER_REQUIREMENTS=true` in the Open WebUI env).

## Install as a local Python package

    pip install "ctxone[openwebui]"

then in your own code::

    from ctxone.integrations.openwebui import Tools, Filter

## Configuration

Both plugins expose two Pydantic `Valves` tiers:

    - `Valves`      — admin-level, one per install. Holds the Hub URL,
                      request timeout, and whether writes are enabled.

    - `UserValves`  — per-user overrides. The user's email becomes the
                      agent ID in commits, so CtxOne's blame view shows
                      exactly which user said what. Users can also pick a
                      private branch, a topic budget, and turn the filter
                      on/off for themselves.
"""

from __future__ import annotations

from typing import Any, Optional

try:
    from pydantic import BaseModel, Field
except ImportError as e:  # pragma: no cover
    raise ImportError(
        "ctxone.integrations.openwebui requires pydantic. Install with:\n"
        '    pip install "ctxone[openwebui]"\n'
        "or, if you pasted this file into Open WebUI, enable "
        "ENABLE_PIP_INSTALL_FRONTMATTER_REQUIREMENTS=true in your "
        "Open WebUI environment and reload the function."
    ) from e

from ctxone import Hub
from ctxone.exceptions import CtxOneError, HubUnreachable


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def _user_email(user: Optional[dict]) -> str:
    """Best-effort agent ID from Open WebUI's __user__ dict."""
    if not user:
        return "openwebui"
    return (
        user.get("email")
        or user.get("name")
        or user.get("id")
        or "openwebui"
    )


def _user_branch(user: Optional[dict], fallback: str) -> str:
    """Pull the user's preferred branch from UserValves if present."""
    if not user:
        return fallback
    valves = user.get("valves")
    if valves is None:
        return fallback
    branch = getattr(valves, "branch", None)
    return branch or fallback


def _last_user_message(messages: list[dict[str, Any]]) -> str:
    """Return the most recent user-role message content."""
    for msg in reversed(messages or []):
        if msg.get("role") == "user":
            content = msg.get("content", "")
            # Open WebUI's `content` can be either a string or a list of
            # multi-modal parts ({type:"text", text:"..."} etc). Join the
            # text parts and ignore anything else.
            if isinstance(content, str):
                return content
            if isinstance(content, list):
                parts: list[str] = []
                for part in content:
                    if isinstance(part, dict) and part.get("type") == "text":
                        parts.append(part.get("text", ""))
                return " ".join(parts)
    return ""


def _last_assistant_message(messages: list[dict[str, Any]]) -> str:
    """Return the most recent assistant-role message content."""
    for msg in reversed(messages or []):
        if msg.get("role") == "assistant":
            content = msg.get("content", "")
            if isinstance(content, str):
                return content
            if isinstance(content, list):
                parts: list[str] = []
                for part in content:
                    if isinstance(part, dict) and part.get("type") == "text":
                        parts.append(part.get("text", ""))
                return " ".join(parts)
    return ""


def _format_recall_as_system_prompt(
    topic: str,
    entries: list[Any],
    savings_ratio: float,
) -> str:
    """Render a RecallResult into a system-prompt-friendly block."""
    if not entries:
        return ""

    lines = [
        "## Relevant memory from CtxOne",
        f"Retrieved for topic: {topic!r}",
        "",
    ]
    for e in entries:
        # MemoryEntry is a dataclass-like object; duck-type it so this
        # also works if the caller passes a raw dict.
        path = getattr(e, "path", None) or (e.get("path") if isinstance(e, dict) else "")
        pinned = getattr(e, "pinned", False) or (e.get("pinned") if isinstance(e, dict) else False)
        value = getattr(e, "value", None) or (e.get("value") if isinstance(e, dict) else None)
        title = getattr(e, "title", None) or (e.get("title") if isinstance(e, dict) else None)
        body = getattr(e, "body", None) or (e.get("body") if isinstance(e, dict) else None)

        tag = "[pinned]" if pinned else "[fact]"
        if title and body:
            lines.append(f"- {tag} **{title}** — {body}")
        elif value:
            lines.append(f"- {tag} {value}")
        elif path:
            lines.append(f"- {tag} {path}")

    if savings_ratio and savings_ratio > 1.0:
        lines.append("")
        lines.append(
            f"_(CtxOne: this retrieval is {savings_ratio:.1f}× "
            "smaller than loading the full memory graph.)_"
        )
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Tools — explicit function calls the model can make
# ---------------------------------------------------------------------------


class Tools:
    """CtxOne memory tools exposed to the model as function calls.

    Open WebUI introspects every method on this class, turns type hints
    into a JSON-schema for function calling, and uses the first docstring
    line as the description. Async or sync both work.
    """

    class Valves(BaseModel):
        hub_url: str = Field(
            default="http://localhost:3001",
            description="CtxOne Hub URL. Default is localhost:3001.",
        )
        default_branch: str = Field(
            default="main",
            description="Branch to read from and write to when the user "
            "has no personal override.",
        )
        timeout_seconds: float = Field(
            default=15.0,
            description="HTTP timeout for Hub requests.",
        )
        allow_writes: bool = Field(
            default=True,
            description="If False, remember/forget return an error instead "
            "of mutating the graph. Useful for read-only demos.",
        )
        recall_budget: int = Field(
            default=1500,
            description="Default token budget passed to recall() when the "
            "model doesn't specify one.",
        )

    class UserValves(BaseModel):
        branch: str = Field(
            default="",
            description="Override the admin default_branch for this user. "
            "Empty string means: fall back to the admin default.",
        )
        remember_importance: str = Field(
            default="medium",
            description="Importance level applied to facts this user "
            "writes via remember(). One of: high, medium, low.",
        )

    def __init__(self) -> None:
        self.valves = self.Valves()

    # -- private plumbing --

    def _hub(self, user: Optional[dict] = None) -> Hub:
        """Build a Hub client for this request.

        The default branch is resolved per-user so private branches
        stay private even when the tool is shared across chats. The
        Hub's X-CtxOne-Session header is set to the user's email (or
        name/id) so per-user token stats stay isolated on the Hub
        side — visible via `GET /api/stats/tokens/<user-email>`.
        """
        email = _user_email(user)
        return Hub(
            server=self.valves.hub_url,
            branch=_user_branch(user, self.valves.default_branch),
            timeout=self.valves.timeout_seconds,
            session_id=email,
            agent_id=email,
        )

    # -- tool methods (exposed to the model) --

    def remember(
        self,
        fact: str,
        context: Optional[str] = None,
        importance: Optional[str] = None,
        __user__: Optional[dict] = None,
    ) -> str:
        """Store a fact in CtxOne long-term memory.

        :param fact: The fact to remember, in natural language.
        :param context: Optional category name (e.g. "licensing",
            "architecture"). Becomes the path prefix. Defaults to
            "facts" when not given.
        :param importance: "high", "medium", or "low". Defaults to the
            user's configured importance.
        """
        if not self.valves.allow_writes:
            return "Error: writes are disabled on this CtxOne installation."

        user_valves = self._user_valves(__user__)
        imp = importance or user_valves.remember_importance
        if imp not in ("high", "medium", "low"):
            return f"Error: importance must be high/medium/low, got {imp!r}"

        try:
            hub = self._hub(__user__)
            result = hub.remember(
                fact,
                importance=imp,
                context=context,
            )
            return (
                f"Remembered. Stored at {result.path} "
                f"on branch {result.ref} as commit {result.commit_id}."
            )
        except HubUnreachable as e:
            return f"Error: CtxOne Hub is unreachable at {self.valves.hub_url}: {e}"
        except CtxOneError as e:
            return f"Error: remember failed: {e}"

    def recall(
        self,
        topic: str,
        budget: Optional[int] = None,
        __user__: Optional[dict] = None,
    ) -> str:
        """Retrieve memories relevant to a topic from CtxOne.

        Always includes pinned memories first, then topic-matched facts,
        respecting a token budget. Returns a human-readable summary.

        :param topic: The topic to search for. Multi-word queries are
            tokenized; stopwords are dropped.
        :param budget: Max tokens to return. Defaults to the admin
            setting (1500).
        """
        try:
            hub = self._hub(__user__)
            result = hub.recall(topic, budget=budget or self.valves.recall_budget)
        except HubUnreachable as e:
            return f"Error: CtxOne Hub is unreachable at {self.valves.hub_url}: {e}"
        except CtxOneError as e:
            return f"Error: recall failed: {e}"

        if not result.results:
            return f"No memories found for {topic!r}."

        block = _format_recall_as_system_prompt(
            topic, result.results, result.ctx_savings_ratio
        )
        return block or "No memories found."

    def forget(
        self,
        path: str,
        reason: Optional[str] = None,
        __user__: Optional[dict] = None,
    ) -> str:
        """Delete a memory at a specific path.

        This writes a rollback commit — the fact is still visible in
        blame history but no longer appears in recall.

        :param path: The exact memory path to forget. Get this from
            recall() output.
        :param reason: Why this is being forgotten. Shows up in blame.
        """
        if not self.valves.allow_writes:
            return "Error: writes are disabled on this CtxOne installation."

        try:
            hub = self._hub(__user__)
            commit_id = hub.forget(
                path,
                reason=reason or f"forgotten via Open WebUI by {_user_email(__user__)}",
            )
            return f"Forgot {path} (rollback commit {commit_id})."
        except HubUnreachable as e:
            return f"Error: CtxOne Hub is unreachable at {self.valves.hub_url}: {e}"
        except CtxOneError as e:
            return f"Error: forget failed: {e}"

    def list_pinned(
        self,
        __user__: Optional[dict] = None,
    ) -> str:
        """List all pinned memories (always-loaded context).

        Pinned memories are injected into every recall regardless of
        topic. Use this to see what's always in your agent's context.
        """
        try:
            hub = self._hub(__user__)
            grouped = hub.pinned_grouped()
        except HubUnreachable as e:
            return f"Error: CtxOne Hub is unreachable at {self.valves.hub_url}: {e}"
        except CtxOneError as e:
            return f"Error: list_pinned failed: {e}"

        if not grouped:
            return "No pinned memories."

        lines = ["## Pinned memories"]
        for source, sections in grouped.items():
            lines.append(f"### {source}")
            for s in sections:
                title = s.get("title", "")
                body = s.get("body", "")
                lines.append(f"- **{title}**: {body}")
        return "\n".join(lines)

    # -- helper --

    def _user_valves(self, user: Optional[dict]) -> "Tools.UserValves":
        """Return UserValves for this user, or defaults."""
        if user is None:
            return self.UserValves()
        v = user.get("valves")
        if isinstance(v, self.UserValves):
            return v
        return self.UserValves()


# ---------------------------------------------------------------------------
# Filter — auto-inject memory into every turn
# ---------------------------------------------------------------------------


class Filter:
    """Auto-injects CtxOne memory into every chat turn.

    This is the plugin that lets CtxOne behave like background memory the
    model doesn't have to ask for. On each inlet, it looks at the user's
    most recent message, calls `hub.recall(topic=that_message)`, and
    prepends the result as a system message so the model sees it before
    generating. On outlet, it optionally captures the assistant's reply
    as a fact for next time (opt-in per user).

    Works with models that don't support tool-calling.
    """

    class Valves(BaseModel):
        # Open WebUI sorts filters by `priority` (lower = earlier). We
        # want memory injection to happen before most other filters so
        # downstream ones see the enriched system prompt.
        priority: int = Field(
            default=-10,
            description="Filter priority. Lower runs earlier.",
        )
        hub_url: str = Field(
            default="http://localhost:3001",
            description="CtxOne Hub URL. Default is localhost:3001.",
        )
        default_branch: str = Field(
            default="main",
            description="Branch to read from and write to when the user "
            "has no personal override.",
        )
        timeout_seconds: float = Field(
            default=8.0,
            description="HTTP timeout for Hub requests. Keep this short — "
            "it's in the hot path of every turn.",
        )
        recall_budget: int = Field(
            default=1500,
            description="Token budget passed to recall() on each inlet.",
        )
        min_query_length: int = Field(
            default=3,
            description="Skip recall when the user message is shorter "
            "than this. Avoids spamming the Hub on one-word prompts.",
        )
        silent_on_error: bool = Field(
            default=True,
            description="If True, Hub errors are logged but don't block "
            "the chat. If False, errors bubble up and fail the turn.",
        )

    class UserValves(BaseModel):
        enabled: bool = Field(
            default=True,
            description="Master switch for this user. Turn off if you "
            "don't want memory injected into your chats.",
        )
        branch: str = Field(
            default="",
            description="Your private branch. Empty string means: use the "
            "admin default.",
        )
        capture_replies: bool = Field(
            default=False,
            description="On outlet, store the assistant's reply as a fact. "
            "Off by default — turning this on makes every conversation "
            "self-teaching but can also fill your memory with junk.",
        )
        capture_importance: str = Field(
            default="low",
            description="Importance level applied to auto-captured replies. "
            "Defaults to 'low' so they never crowd out real facts.",
        )

    def __init__(self) -> None:
        self.valves = self.Valves()
        # Render a per-chat toggle switch in the UI. Users can disable
        # the filter without touching their UserValves.
        self.toggle = True

    # -- lifecycle hooks --

    async def inlet(
        self,
        body: dict,
        __user__: Optional[dict] = None,
    ) -> dict:
        """Run before every model call. Injects relevant memories.

        Open WebUI passes the full OpenAI-style request body here. We
        mutate `body["messages"]` in place by prepending a system
        message, then return it.
        """
        user_valves = self._user_valves(__user__)
        if not user_valves.enabled:
            return body

        messages = body.get("messages", [])
        query = _last_user_message(messages)
        if len(query) < self.valves.min_query_length:
            return body

        try:
            hub = self._hub(__user__)
            result = hub.recall(query, budget=self.valves.recall_budget)
        except HubUnreachable:
            if self.valves.silent_on_error:
                return body
            raise
        except CtxOneError:
            if self.valves.silent_on_error:
                return body
            raise

        if not result.results:
            return body

        block = _format_recall_as_system_prompt(
            query, result.results, result.ctx_savings_ratio
        )
        if not block:
            return body

        # Prepend as a NEW system message so we don't stomp on any
        # system prompt the model already has.
        messages.insert(0, {"role": "system", "content": block})
        body["messages"] = messages
        return body

    async def outlet(
        self,
        body: dict,
        __user__: Optional[dict] = None,
    ) -> dict:
        """Run after the full response is assembled.

        If the user has `capture_replies` on, store the assistant's
        reply as a fact for future recall.
        """
        user_valves = self._user_valves(__user__)
        if not user_valves.enabled or not user_valves.capture_replies:
            return body

        messages = body.get("messages", [])
        reply = _last_assistant_message(messages)
        if not reply or len(reply) < 16:
            # Too short to be a useful fact.
            return body

        try:
            hub = self._hub(__user__)
            hub.remember(
                reply,
                importance=user_valves.capture_importance,
                context=f"openwebui/{_user_email(__user__)}",
            )
        except (HubUnreachable, CtxOneError):
            # Writes are best-effort — never fail a chat because of
            # a memory-capture error.
            pass
        return body

    def stream(self, event: dict) -> dict:
        """Stream pass-through. No transformation.

        Implemented so Open WebUI knows this filter is stream-aware
        (otherwise it's skipped during streaming responses). Override
        if you want to redact or transform chunks mid-flight.
        """
        return event

    # -- private --

    def _hub(self, user: Optional[dict] = None) -> Hub:
        """Build a Hub client for this request, with the user's agent ID.

        Sets X-CtxOne-Session so the Hub's per-session token stats
        show this user's usage separately from everyone else's.
        """
        email = _user_email(user)
        return Hub(
            server=self.valves.hub_url,
            branch=_user_branch(user, self.valves.default_branch),
            timeout=self.valves.timeout_seconds,
            session_id=email,
            agent_id=email,
        )

    def _user_valves(self, user: Optional[dict]) -> "Filter.UserValves":
        """Return UserValves for this user, or defaults."""
        if user is None:
            return self.UserValves()
        v = user.get("valves")
        if isinstance(v, self.UserValves):
            return v
        return self.UserValves()
