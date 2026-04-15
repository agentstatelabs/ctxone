"""Tests for the Open WebUI integration.

These mock the Hub client so we don't need a running CtxOne instance.
The goal is to verify the Tool and Filter classes:
    - Build Hub clients with the right URL / branch / timeout
    - Honor UserValves overrides (branch, enabled, capture_replies)
    - Fail gracefully on HubUnreachable / CtxOneError
    - Inject system messages in the right shape
    - Skip writes when disabled

We also smoke-test the shared helpers (_user_email, _last_user_message,
_format_recall_as_system_prompt).
"""

from __future__ import annotations

import asyncio
from unittest.mock import MagicMock, patch

import pytest

# Pydantic is required for the integration. If it's not installed, skip
# the whole module — this way the test run doesn't break for users who
# installed ctxone without the [openwebui] extra.
pytest.importorskip("pydantic")

from ctxone.exceptions import CtxOneError, HubUnreachable
from ctxone.integrations.openwebui import (
    Filter,
    Tools,
    _format_recall_as_system_prompt,
    _last_assistant_message,
    _last_user_message,
    _user_branch,
    _user_email,
)
from ctxone.types import MemoryEntry, RecallResult, RememberResult


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def fake_recall_result(entries=None, ratio=10.0):
    return RecallResult(
        topic="x",
        results=entries or [],
        pinned_count=0,
        topic_matches=len(entries or []),
        ctx_tokens_sent=100,
        ctx_tokens_estimated_flat=1000,
        ctx_savings_ratio=ratio,
        ref="main",
        raw={},
    )


def fake_memory_entry(value="BSL-1.1", pinned=False, path="/memory/legal/x"):
    return MemoryEntry(
        path=path,
        pinned=pinned,
        value=value,
        title=None,
        body=None,
        score=0.9,
        full_match=True,
    )


def fake_pinned_entry(title="Vision", body="ship it"):
    return MemoryEntry(
        path="/memory/pinned/src/vision",
        pinned=True,
        value=None,
        title=title,
        body=body,
        score=None,
        full_match=None,
    )


def run_async(coro):
    """Tiny wrapper so the sync test functions can drive async hooks.

    Uses `asyncio.run` rather than `get_event_loop` for Python 3.14
    compatibility — the latter raises when no loop exists yet.
    """
    return asyncio.run(coro)


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def test_user_email_falls_back_to_default_when_no_user():
    assert _user_email(None) == "openwebui"
    assert _user_email({}) == "openwebui"


def test_user_email_prefers_email_over_name_and_id():
    assert _user_email({"email": "a@b", "name": "alice", "id": "1"}) == "a@b"
    assert _user_email({"name": "alice", "id": "1"}) == "alice"
    assert _user_email({"id": "1"}) == "1"


def test_user_branch_uses_valves_branch_when_present():
    valves = Tools.UserValves(branch="alice-private")
    assert _user_branch({"valves": valves}, "main") == "alice-private"


def test_user_branch_falls_back_when_valves_branch_empty():
    valves = Tools.UserValves(branch="")
    assert _user_branch({"valves": valves}, "main") == "main"
    assert _user_branch(None, "main") == "main"
    assert _user_branch({}, "main") == "main"


def test_last_user_message_extracts_plain_string():
    messages = [
        {"role": "system", "content": "sys"},
        {"role": "user", "content": "hello"},
        {"role": "assistant", "content": "hi"},
        {"role": "user", "content": "how are you"},
    ]
    assert _last_user_message(messages) == "how are you"


def test_last_user_message_joins_multimodal_text_parts():
    messages = [
        {
            "role": "user",
            "content": [
                {"type": "text", "text": "what is"},
                {"type": "image_url", "image_url": "http://..."},
                {"type": "text", "text": "in this image"},
            ],
        },
    ]
    assert _last_user_message(messages) == "what is in this image"


def test_last_user_message_returns_empty_when_none():
    assert _last_user_message([]) == ""
    assert _last_user_message([{"role": "assistant", "content": "hi"}]) == ""


def test_last_assistant_message_extracts_plain_string():
    messages = [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "hello there"},
    ]
    assert _last_assistant_message(messages) == "hello there"


def test_format_recall_empty_returns_empty_string():
    assert _format_recall_as_system_prompt("x", [], 0.0) == ""


def test_format_recall_renders_entries_with_tags():
    entries = [
        fake_memory_entry(value="BSL-1.1 licensing", path="/memory/legal/x"),
        fake_pinned_entry(title="Vision", body="ship fast"),
    ]
    block = _format_recall_as_system_prompt("licensing", entries, 12.3)
    assert "## Relevant memory from CtxOne" in block
    assert "'licensing'" in block
    assert "[fact]" in block
    assert "BSL-1.1" in block
    assert "[pinned]" in block
    assert "Vision" in block
    assert "ship fast" in block
    assert "12.3×" in block


def test_format_recall_hides_ratio_when_below_one():
    entries = [fake_memory_entry()]
    block = _format_recall_as_system_prompt("x", entries, 0.8)
    assert "×" not in block


# ---------------------------------------------------------------------------
# Tools — remember / recall / forget / list_pinned
# ---------------------------------------------------------------------------


@pytest.fixture
def tools():
    t = Tools()
    t.valves.hub_url = "http://fake:3001"
    return t


def test_tools_remember_uses_default_importance(tools):
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.remember.return_value = RememberResult(
            path="/memory/facts/abc",
            commit_id="sg_dead",
            ref="main",
        )
        mock_hub_method.return_value = mock_hub

        result = tools.remember("BSL is cool")
        mock_hub.remember.assert_called_once_with(
            "BSL is cool", importance="medium", context=None
        )
        assert "Remembered" in result
        assert "/memory/facts/abc" in result


def test_tools_remember_rejects_writes_when_disabled(tools):
    tools.valves.allow_writes = False
    result = tools.remember("doomed")
    assert "Error" in result
    assert "disabled" in result


def test_tools_remember_rejects_invalid_importance(tools):
    result = tools.remember("x", importance="critical")
    assert "Error" in result
    assert "importance" in result


def test_tools_remember_handles_hub_unreachable(tools):
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.remember.side_effect = HubUnreachable("connection refused")
        mock_hub_method.return_value = mock_hub

        result = tools.remember("x")
        assert "Error" in result
        assert "unreachable" in result


def test_tools_recall_formats_results_as_markdown(tools):
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.recall.return_value = fake_recall_result(
            entries=[fake_memory_entry(value="CtxOne uses BSL-1.1")]
        )
        mock_hub_method.return_value = mock_hub

        result = tools.recall("licensing")
        assert "Relevant memory from CtxOne" in result
        assert "BSL-1.1" in result


def test_tools_recall_returns_human_message_when_empty(tools):
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.recall.return_value = fake_recall_result(entries=[])
        mock_hub_method.return_value = mock_hub

        result = tools.recall("nothing-here")
        assert "No memories found" in result


def test_tools_recall_uses_default_budget_when_none_given(tools):
    tools.valves.recall_budget = 2000
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.recall.return_value = fake_recall_result(entries=[])
        mock_hub_method.return_value = mock_hub

        tools.recall("x")
        mock_hub.recall.assert_called_once_with("x", budget=2000)


def test_tools_forget_writes_rollback_with_user_reason(tools):
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.forget.return_value = "sg_roll"
        mock_hub_method.return_value = mock_hub

        result = tools.forget(
            "/memory/legal/x",
            __user__={"email": "alice@example.com"},
        )
        args, kwargs = mock_hub.forget.call_args
        assert args[0] == "/memory/legal/x"
        assert "alice@example.com" in kwargs["reason"]
        assert "sg_roll" in result


def test_tools_list_pinned_handles_empty(tools):
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.pinned_grouped.return_value = {}
        mock_hub_method.return_value = mock_hub

        result = tools.list_pinned()
        assert "No pinned memories" in result


def test_tools_list_pinned_groups_by_source(tools):
    with patch.object(Tools, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.pinned_grouped.return_value = {
            "project": [
                {"title": "Vision", "body": "ship fast"},
                {"title": "Stack", "body": "Rust + SvelteKit"},
            ],
        }
        mock_hub_method.return_value = mock_hub

        result = tools.list_pinned()
        assert "## Pinned memories" in result
        assert "### project" in result
        assert "Vision" in result
        assert "ship fast" in result
        assert "Stack" in result


def test_tools_hub_respects_user_branch_override(tools):
    with patch("ctxone.integrations.openwebui.Hub") as mock_hub_class:
        user = {"valves": Tools.UserValves(branch="alice-private")}
        tools._hub(user)
        _, kwargs = mock_hub_class.call_args
        assert kwargs["branch"] == "alice-private"


# ---------------------------------------------------------------------------
# Filter — inlet / outlet / stream
# ---------------------------------------------------------------------------


@pytest.fixture
def filt():
    f = Filter()
    f.valves.hub_url = "http://fake:3001"
    # Short min_query_length so tests don't have to pad their prompts
    f.valves.min_query_length = 2
    return f


def test_filter_inlet_skips_when_user_disabled(filt):
    with patch.object(Filter, "_hub") as mock_hub_method:
        body = {"messages": [{"role": "user", "content": "licensing question"}]}
        user = {"valves": Filter.UserValves(enabled=False)}
        out = run_async(filt.inlet(body, __user__=user))
        assert out is body
        mock_hub_method.assert_not_called()


def test_filter_inlet_skips_when_message_too_short(filt):
    filt.valves.min_query_length = 10
    with patch.object(Filter, "_hub") as mock_hub_method:
        body = {"messages": [{"role": "user", "content": "hi"}]}
        out = run_async(filt.inlet(body))
        assert out is body
        mock_hub_method.assert_not_called()


def test_filter_inlet_injects_system_message_on_match(filt):
    with patch.object(Filter, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.recall.return_value = fake_recall_result(
            entries=[fake_memory_entry(value="CtxOne uses BSL-1.1")],
            ratio=15.0,
        )
        mock_hub_method.return_value = mock_hub

        body = {
            "messages": [
                {"role": "user", "content": "What licensing do we use?"}
            ]
        }
        out = run_async(filt.inlet(body))

        # New system message prepended with the memory block
        assert out["messages"][0]["role"] == "system"
        assert "BSL-1.1" in out["messages"][0]["content"]
        assert "Relevant memory from CtxOne" in out["messages"][0]["content"]

        # Original user message still there after the injection
        assert out["messages"][1]["role"] == "user"

        # Hub was called with the user's message as the topic
        mock_hub.recall.assert_called_once()
        _, kwargs = mock_hub.recall.call_args
        assert "budget" in kwargs


def test_filter_inlet_skips_injection_when_no_matches(filt):
    with patch.object(Filter, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.recall.return_value = fake_recall_result(entries=[])
        mock_hub_method.return_value = mock_hub

        body = {"messages": [{"role": "user", "content": "random question"}]}
        out = run_async(filt.inlet(body))
        # No system message prepended — original message at index 0
        assert out["messages"][0]["role"] == "user"


def test_filter_inlet_swallows_hub_errors_when_silent(filt):
    filt.valves.silent_on_error = True
    with patch.object(Filter, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.recall.side_effect = HubUnreachable("down")
        mock_hub_method.return_value = mock_hub

        body = {"messages": [{"role": "user", "content": "question"}]}
        out = run_async(filt.inlet(body))
        # Body returned unchanged, no exception
        assert out["messages"][0]["role"] == "user"


def test_filter_inlet_propagates_hub_errors_when_not_silent(filt):
    filt.valves.silent_on_error = False
    with patch.object(Filter, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.recall.side_effect = HubUnreachable("down")
        mock_hub_method.return_value = mock_hub

        body = {"messages": [{"role": "user", "content": "question"}]}
        with pytest.raises(HubUnreachable):
            run_async(filt.inlet(body))


def test_filter_outlet_skips_when_capture_disabled(filt):
    with patch.object(Filter, "_hub") as mock_hub_method:
        body = {
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello, i am a long answer"},
            ]
        }
        # UserValves defaults: capture_replies=False
        out = run_async(filt.outlet(body))
        assert out is body
        mock_hub_method.assert_not_called()


def test_filter_outlet_captures_reply_when_enabled(filt):
    with patch.object(Filter, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub_method.return_value = mock_hub

        body = {
            "messages": [
                {"role": "user", "content": "tell me about BSL"},
                {
                    "role": "assistant",
                    "content": "BSL is a source-available license that converts to Apache 2 after 4 years",
                },
            ]
        }
        user = {
            "email": "alice@example.com",
            "valves": Filter.UserValves(
                enabled=True,
                capture_replies=True,
                capture_importance="low",
            ),
        }
        run_async(filt.outlet(body, __user__=user))

        mock_hub.remember.assert_called_once()
        args, kwargs = mock_hub.remember.call_args
        assert "BSL" in args[0]
        assert kwargs["importance"] == "low"
        assert "alice@example.com" in kwargs["context"]


def test_filter_outlet_skips_short_replies(filt):
    with patch.object(Filter, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub_method.return_value = mock_hub

        body = {
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "ok"},
            ]
        }
        user = {"valves": Filter.UserValves(enabled=True, capture_replies=True)}
        run_async(filt.outlet(body, __user__=user))
        mock_hub.remember.assert_not_called()


def test_filter_outlet_swallows_capture_errors(filt):
    with patch.object(Filter, "_hub") as mock_hub_method:
        mock_hub = MagicMock()
        mock_hub.remember.side_effect = CtxOneError("something broke")
        mock_hub_method.return_value = mock_hub

        body = {
            "messages": [
                {"role": "user", "content": "question"},
                {
                    "role": "assistant",
                    "content": "this is a plenty-long answer to capture",
                },
            ]
        }
        user = {"valves": Filter.UserValves(enabled=True, capture_replies=True)}
        # Should not raise
        out = run_async(filt.outlet(body, __user__=user))
        assert out is body


def test_filter_stream_is_passthrough(filt):
    event = {"delta": "hello"}
    assert filt.stream(event) is event


def test_filter_has_toggle_attr_for_per_chat_switch(filt):
    # Open WebUI looks at self.toggle to render a per-chat UI toggle
    assert filt.toggle is True
