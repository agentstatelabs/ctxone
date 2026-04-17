"""Unit tests for the Hub client using a mocked requests.Session.

These don't need a running Hub; they verify the client builds the right
URLs, handles the right status codes, and parses responses into typed
dataclasses correctly.
"""

from __future__ import annotations

from unittest.mock import MagicMock
from urllib.parse import urlparse, parse_qs

import pytest

from ctxone import (
    CtxOneError,
    Hub,
    HubUnreachable,
    MergeConflict,
    NotFound,
)


def mock_response(status_code=200, json_body=None, content_type="application/json"):
    """Build a fake requests.Response-ish object."""
    resp = MagicMock()
    resp.status_code = status_code
    resp.ok = 200 <= status_code < 300
    resp.headers = {"content-type": content_type}
    resp.json.return_value = json_body or {}
    resp.text = str(json_body or "")
    resp.url = "http://fake"
    return resp


def make_hub(session):
    return Hub(server="http://fake:3001", session=session)


# -------- remember / forget --------

def test_remember_posts_correct_body():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={
            "status": "ok",
            "ref": "main",
            "path": "/memory/legal/abc",
            "commit_id": "sg_deadbeef",
        }
    )
    hub = make_hub(session)

    result = hub.remember(
        "BSL-1.1", importance="high", context="legal", tags=["law"]
    )

    args, kwargs = session.post.call_args
    assert args[0] == "http://fake:3001/api/memory/remember"
    body = kwargs["json"]
    assert body["fact"] == "BSL-1.1"
    assert body["importance"] == "high"
    assert body["context"] == "legal"
    assert body["tags"] == ["law"]
    assert body["ref"] == "main"

    assert result.path == "/memory/legal/abc"
    assert result.commit_id == "sg_deadbeef"


def test_remember_minimal_body_omits_optional_fields():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={"status": "ok", "path": "/memory/facts/1", "commit_id": "sg_1"}
    )
    hub = make_hub(session)

    hub.remember("a fact")

    body = session.post.call_args.kwargs["json"]
    assert body["fact"] == "a fact"
    assert body["importance"] == "medium"
    assert "context" not in body
    assert "tags" not in body


def test_remember_override_ref():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={"status": "ok", "path": "/x", "commit_id": "sg_1", "ref": "exp"}
    )
    hub = make_hub(session)

    hub.remember("x", ref="exp")
    body = session.post.call_args.kwargs["json"]
    assert body["ref"] == "exp"


def test_forget_returns_commit_id():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={"status": "ok", "commit_id": "sg_forget"}
    )
    hub = make_hub(session)

    commit = hub.forget("/memory/facts/1", reason="cleanup")
    assert commit == "sg_forget"

    body = session.post.call_args.kwargs["json"]
    assert body["path"] == "/memory/facts/1"
    assert body["reason"] == "cleanup"


# -------- recall --------

def test_recall_parses_results_and_stats():
    session = MagicMock()
    session.get.return_value = mock_response(
        json_body={
            "topic": "licensing",
            "ref": "main",
            "results": [
                {
                    "path": "/memory/pinned/proj/vision",
                    "title": "Vision",
                    "body": "We use BSL",
                    "pinned": True,
                },
                {
                    "path": "/memory/legal/abc",
                    "value": "BSL-1.1 for everything",
                    "pinned": False,
                    "score": 2,
                    "full_match": True,
                },
            ],
            "pinned_count": 1,
            "topic_matches": 1,
            "ctx_tokens_sent": 50,
            "ctx_tokens_estimated_flat": 500,
            "ctx_savings_ratio": 10.0,
        }
    )
    hub = make_hub(session)

    result = hub.recall("licensing", budget=1000)

    assert result.topic == "licensing"
    assert result.pinned_count == 1
    assert result.topic_matches == 1
    assert result.ctx_savings_ratio == 10.0
    assert len(result.results) == 2

    pinned = result.results[0]
    assert pinned.pinned is True
    assert pinned.title == "Vision"
    assert pinned.body == "We use BSL"

    topic = result.results[1]
    assert topic.pinned is False
    assert topic.value == "BSL-1.1 for everything"
    assert topic.score == 2
    assert topic.full_match is True

    # Verify query params
    call = session.get.call_args
    params = call.kwargs["params"]
    assert params["topic"] == "licensing"
    assert params["budget"] == "1000"
    assert params["ref"] == "main"


# -------- prime / prime_markdown --------

def test_prime_passes_sections_through():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={
            "status": "ok",
            "source": "proj",
            "pinned": True,
            "sections_written": 2,
            "paths": ["/memory/pinned/proj/a", "/memory/pinned/proj/b"],
        }
    )
    hub = make_hub(session)

    sections = [
        {"title": "A", "body": "body a"},
        {"title": "B", "body": "body b"},
    ]
    result = hub.prime("proj", sections, pinned=True)

    assert result.source == "proj"
    assert result.pinned is True
    assert result.sections_written == 2

    body = session.post.call_args.kwargs["json"]
    assert body["source"] == "proj"
    assert body["pinned"] is True
    assert body["sections"] == sections


def test_prime_markdown_parses_and_submits():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={
            "status": "ok",
            "source": "readme",
            "pinned": False,
            "sections_written": 2,
            "paths": [],
        }
    )
    hub = make_hub(session)

    md = "# One\nfirst body\n\n# Two\nsecond body\n"
    hub.prime_markdown("readme", md)

    body = session.post.call_args.kwargs["json"]
    assert len(body["sections"]) == 2
    assert body["sections"][0]["title"] == "One"
    assert body["sections"][1]["title"] == "Two"


def test_prime_markdown_rejects_empty_content():
    hub = Hub(session=MagicMock())
    with pytest.raises(CtxOneError):
        hub.prime_markdown("empty", "")


def test_prime_markdown_treats_plain_text_as_intro_section():
    # Content without any H1/H2 becomes one "Intro" section.
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={
            "status": "ok",
            "source": "notes",
            "pinned": False,
            "sections_written": 1,
            "paths": [],
        }
    )
    hub = make_hub(session)

    hub.prime_markdown("notes", "just some plain notes, no headings")

    body = session.post.call_args.kwargs["json"]
    assert len(body["sections"]) == 1
    assert body["sections"][0]["title"] == "Intro"


# -------- pinned_grouped --------

def test_pinned_grouped_pairs_title_body():
    session = MagicMock()
    session.get.return_value = mock_response(
        json_body=[
            {"path": "/memory/pinned/proj/vision/title", "value": "Vision"},
            {"path": "/memory/pinned/proj/vision/body", "value": "The pitch"},
            {"path": "/memory/pinned/proj/roadmap/title", "value": "Roadmap"},
            {"path": "/memory/pinned/proj/roadmap/body", "value": "The plan"},
        ]
    )
    hub = make_hub(session)

    grouped = hub.pinned_grouped()
    assert "proj" in grouped
    assert len(grouped["proj"]) == 2
    titles = {s["title"] for s in grouped["proj"]}
    assert titles == {"Vision", "Roadmap"}


# -------- error handling --------

def test_not_found_raises():
    session = MagicMock()
    session.get.return_value = mock_response(status_code=404, content_type="text/plain")
    session.get.return_value.text = "not found"
    hub = make_hub(session)

    with pytest.raises(NotFound):
        hub.recall("foo")


def test_merge_conflict_raises():
    session = MagicMock()
    session.post.return_value = mock_response(
        status_code=409,
        json_body={"conflicts": [{"path": "/x", "reason": "both modified"}]},
    )
    hub = make_hub(session)

    with pytest.raises(MergeConflict) as exc_info:
        hub.merge("exp", into="main")

    assert len(exc_info.value.conflicts) == 1
    assert exc_info.value.conflicts[0]["path"] == "/x"


def test_connection_error_raises_unreachable():
    import requests as req

    session = MagicMock()
    session.get.side_effect = req.ConnectionError("connection refused")
    hub = make_hub(session)

    with pytest.raises(HubUnreachable):
        hub.log()


def test_server_error_raises_ctxone_error():
    session = MagicMock()
    session.get.return_value = mock_response(status_code=500, content_type="text/plain")
    session.get.return_value.text = "internal error"
    hub = make_hub(session)

    with pytest.raises(CtxOneError):
        hub.recall("foo")


# -------- is_reachable --------

def test_is_reachable_true_on_200():
    session = MagicMock()
    session.get.return_value = mock_response(json_body={"status": "ok"})
    hub = make_hub(session)
    assert hub.is_reachable() is True


def test_is_reachable_false_on_connection_error():
    import requests as req

    session = MagicMock()
    session.get.side_effect = req.ConnectionError("refused")
    hub = make_hub(session)
    assert hub.is_reachable() is False


# -------- session_id (X-CTXone-Session header) --------

def test_session_id_defaults_to_none_and_sends_no_header():
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = make_hub(session)

    hub.ls("/")
    _, kwargs = session.get.call_args
    headers = kwargs.get("headers") or {}
    assert "X-CTXone-Session" not in headers


def test_session_id_sent_as_header_on_get():
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = Hub(
        server="http://fake:3001", session=session, session_id="alice@example.com"
    )

    hub.ls("/")
    _, kwargs = session.get.call_args
    assert kwargs["headers"]["X-CTXone-Session"] == "alice@example.com"


def test_session_id_sent_as_header_on_post():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={
            "status": "ok",
            "ref": "main",
            "path": "/memory/x",
            "commit_id": "sg_abc",
        }
    )
    hub = Hub(
        server="http://fake:3001", session=session, session_id="bob@example.com"
    )

    hub.remember("fact")
    _, kwargs = session.post.call_args
    assert kwargs["headers"]["X-CTXone-Session"] == "bob@example.com"


def test_session_id_read_from_env_when_not_explicit(monkeypatch):
    monkeypatch.setenv("CTX_SESSION_ID", "env-session")
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = Hub(server="http://fake:3001", session=session)

    hub.ls("/")
    _, kwargs = session.get.call_args
    assert kwargs["headers"]["X-CTXone-Session"] == "env-session"


def test_explicit_session_id_overrides_env(monkeypatch):
    monkeypatch.setenv("CTX_SESSION_ID", "env-session")
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = Hub(
        server="http://fake:3001", session=session, session_id="explicit"
    )

    hub.ls("/")
    _, kwargs = session.get.call_args
    assert kwargs["headers"]["X-CTXone-Session"] == "explicit"


# -------- agent_id (X-CTXone-Agent header) --------

def test_agent_id_defaults_to_none_and_sends_no_header(monkeypatch):
    # Guard against any CTX_AGENT_ID left in the environment
    monkeypatch.delenv("CTX_AGENT_ID", raising=False)
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = make_hub(session)

    hub.ls("/")
    _, kwargs = session.get.call_args
    headers = kwargs.get("headers") or {}
    assert "X-CTXone-Agent" not in headers


def test_agent_id_sent_as_header_on_get(monkeypatch):
    monkeypatch.delenv("CTX_AGENT_ID", raising=False)
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = Hub(server="http://fake:3001", session=session, agent_id="claude-code")

    hub.ls("/")
    _, kwargs = session.get.call_args
    assert kwargs["headers"]["X-CTXone-Agent"] == "claude-code"


def test_agent_id_sent_as_header_on_post(monkeypatch):
    monkeypatch.delenv("CTX_AGENT_ID", raising=False)
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={
            "status": "ok",
            "ref": "main",
            "path": "/memory/x",
            "commit_id": "sg_abc",
        }
    )
    hub = Hub(server="http://fake:3001", session=session, agent_id="cursor")

    hub.remember("fact")
    _, kwargs = session.post.call_args
    assert kwargs["headers"]["X-CTXone-Agent"] == "cursor"


def test_agent_id_read_from_env_when_not_explicit(monkeypatch):
    monkeypatch.setenv("CTX_AGENT_ID", "env-agent")
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = Hub(server="http://fake:3001", session=session)

    hub.ls("/")
    _, kwargs = session.get.call_args
    assert kwargs["headers"]["X-CTXone-Agent"] == "env-agent"


def test_explicit_agent_id_overrides_env(monkeypatch):
    monkeypatch.setenv("CTX_AGENT_ID", "env-agent")
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = Hub(server="http://fake:3001", session=session, agent_id="explicit-agent")

    hub.ls("/")
    _, kwargs = session.get.call_args
    assert kwargs["headers"]["X-CTXone-Agent"] == "explicit-agent"


def test_both_session_and_agent_headers_sent_together(monkeypatch):
    monkeypatch.delenv("CTX_AGENT_ID", raising=False)
    monkeypatch.delenv("CTX_SESSION_ID", raising=False)
    session = MagicMock()
    session.get.return_value = mock_response(json_body=[])
    hub = Hub(
        server="http://fake:3001",
        session=session,
        session_id="alice@example.com",
        agent_id="claude-code",
    )

    hub.ls("/")
    _, kwargs = session.get.call_args
    assert kwargs["headers"]["X-CTXone-Session"] == "alice@example.com"
    assert kwargs["headers"]["X-CTXone-Agent"] == "claude-code"


# -------- record_usage (LLM usage capture) --------

def _snapshot_body(**overrides):
    """Build a SessionSnapshot-shaped JSON body with sensible defaults."""
    body = {
        "session_id": "default",
        "session_tokens_used": 0,
        "session_tokens_saved": 0,
        "total_graph_size_chars": 0,
        "total_graph_size_tokens": 0,
        "cumulative_ratio": 0.0,
        "llm_input_tokens": 0,
        "llm_output_tokens": 0,
        "llm_cache_read_tokens": 0,
        "llm_cache_create_tokens": 0,
        "llm_call_count": 0,
        "last_model": None,
        "last_provider": None,
    }
    body.update(overrides)
    return body


def test_record_usage_posts_to_llm_usage_endpoint():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_snapshot_body(
            llm_input_tokens=100, llm_output_tokens=50, llm_call_count=1
        )
    )
    hub = make_hub(session)

    snap = hub.record_usage(input_tokens=100, output_tokens=50)

    args, kwargs = session.post.call_args
    assert args[0] == "http://fake:3001/api/stats/llm_usage"
    body = kwargs["json"]
    assert body["input_tokens"] == 100
    assert body["output_tokens"] == 50
    # Defaults present on the wire so the Hub doesn't have to infer
    assert body["cache_read_tokens"] == 0
    assert body["cache_create_tokens"] == 0
    # Optional model/provider omitted when not provided
    assert "model" not in body
    assert "provider" not in body

    assert snap.llm_input_tokens == 100
    assert snap.llm_output_tokens == 50
    assert snap.llm_call_count == 1


def test_record_usage_includes_cache_and_model_when_provided():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_snapshot_body(
            llm_input_tokens=2400,
            llm_output_tokens=450,
            llm_cache_read_tokens=1800,
            llm_cache_create_tokens=600,
            llm_call_count=1,
            last_model="claude-sonnet-4.5",
            last_provider="anthropic",
        )
    )
    hub = make_hub(session)

    snap = hub.record_usage(
        input_tokens=2400,
        output_tokens=450,
        cache_read_tokens=1800,
        cache_create_tokens=600,
        model="claude-sonnet-4.5",
        provider="anthropic",
    )

    body = session.post.call_args.kwargs["json"]
    assert body["cache_read_tokens"] == 1800
    assert body["cache_create_tokens"] == 600
    assert body["model"] == "claude-sonnet-4.5"
    assert body["provider"] == "anthropic"

    assert snap.last_model == "claude-sonnet-4.5"
    assert snap.last_provider == "anthropic"
    assert snap.llm_cache_read_tokens == 1800


def test_record_usage_sends_session_header():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_snapshot_body(session_id="alice@example.com")
    )
    hub = Hub(
        server="http://fake:3001",
        session=session,
        session_id="alice@example.com",
    )

    hub.record_usage(input_tokens=10, output_tokens=5)

    _, kwargs = session.post.call_args
    assert kwargs["headers"]["X-CTXone-Session"] == "alice@example.com"


def test_record_usage_from_anthropic_pulls_usage_fields():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_snapshot_body(
            llm_input_tokens=1200,
            llm_output_tokens=300,
            llm_cache_read_tokens=800,
            llm_cache_create_tokens=100,
            llm_call_count=1,
            last_model="claude-sonnet-4.5",
            last_provider="anthropic",
        )
    )
    hub = make_hub(session)

    # Fake Anthropic usage object
    usage = MagicMock()
    usage.input_tokens = 1200
    usage.output_tokens = 300
    usage.cache_read_input_tokens = 800
    usage.cache_creation_input_tokens = 100

    snap = hub.record_usage_from_anthropic(usage, model="claude-sonnet-4.5")

    body = session.post.call_args.kwargs["json"]
    assert body["input_tokens"] == 1200
    assert body["output_tokens"] == 300
    assert body["cache_read_tokens"] == 800
    assert body["cache_create_tokens"] == 100
    assert body["model"] == "claude-sonnet-4.5"
    assert body["provider"] == "anthropic"

    assert snap.last_provider == "anthropic"


def test_record_usage_from_anthropic_handles_missing_cache_fields():
    """Old-style Anthropic usage objects without cache fields should
    still work — getattr returns 0, None coalesces to 0."""
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_snapshot_body(llm_input_tokens=50, llm_output_tokens=25)
    )
    hub = make_hub(session)

    class BareUsage:
        input_tokens = 50
        output_tokens = 25
        # No cache_read_input_tokens / cache_creation_input_tokens

    hub.record_usage_from_anthropic(BareUsage())

    body = session.post.call_args.kwargs["json"]
    assert body["input_tokens"] == 50
    assert body["output_tokens"] == 25
    assert body["cache_read_tokens"] == 0
    assert body["cache_create_tokens"] == 0


def test_record_usage_from_anthropic_coerces_none_cache_fields_to_zero():
    """The Anthropic SDK sometimes returns None for cache fields when
    caching wasn't used — we should turn those into 0, not crash."""
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_snapshot_body(llm_input_tokens=50, llm_output_tokens=25)
    )
    hub = make_hub(session)

    usage = MagicMock()
    usage.input_tokens = 50
    usage.output_tokens = 25
    usage.cache_read_input_tokens = None
    usage.cache_creation_input_tokens = None

    hub.record_usage_from_anthropic(usage)

    body = session.post.call_args.kwargs["json"]
    assert body["cache_read_tokens"] == 0
    assert body["cache_create_tokens"] == 0


def test_record_usage_returns_default_snapshot_on_empty_response():
    """Defensive: a response that somehow omits LLM fields should
    still produce a SessionSnapshot with zeros instead of throwing."""
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body={"session_id": "sparse"}
    )
    hub = make_hub(session)

    snap = hub.record_usage(input_tokens=1, output_tokens=1)
    assert snap.session_id == "sparse"
    assert snap.llm_input_tokens == 0
    assert snap.last_model is None


# -------- plans --------

def _plan_body(name="p1", **overrides):
    body = {
        "name": name,
        "description": None,
        "status": "active",
        "created_at": "2026-04-15T18:32:00Z",
        "created_by": "test",
        "archived_at": None,
        "task_counts": {
            "pending": 0,
            "in_progress": 0,
            "done": 0,
            "abandoned": 0,
            "total": 0,
        },
    }
    body.update(overrides)
    return body


def _task_body(id="t-001", **overrides):
    body = {
        "id": id,
        "title": "hello",
        "status": "pending",
        "priority": "medium",
        "parent_id": None,
        "blocked_by": [],
        "assigned_to": None,
        "created_at": "2026-04-15T18:32:00Z",
        "created_by": "test",
        "started_at": None,
        "started_by": None,
        "completed_at": None,
        "completed_by": None,
        "abandoned_at": None,
        "abandoned_reason": None,
        "proof": None,
    }
    body.update(overrides)
    return body


def test_plan_new_posts_body():
    session = MagicMock()
    session.post.return_value = mock_response(
        status_code=201, json_body=_plan_body(name="website-v2")
    )
    hub = make_hub(session)

    plan = hub.plan_new("website-v2", description="pivot")

    args, kwargs = session.post.call_args
    assert args[0] == "http://fake:3001/api/plans"
    assert kwargs["json"]["name"] == "website-v2"
    assert kwargs["json"]["description"] == "pivot"
    assert kwargs["json"]["ref"] == "main"
    assert plan.name == "website-v2"
    assert plan.status == "active"


def test_plan_add_posts_body_with_assignment():
    session = MagicMock()
    session.post.return_value = mock_response(
        status_code=201,
        json_body=_task_body(
            id="t-005",
            priority="high",
            assigned_to="codex",
        ),
    )
    hub = make_hub(session)

    task = hub.plan_add(
        "website-v2",
        "Rewrite hero",
        priority="high",
        assigned_to="codex",
        blocked_by=["t-001"],
    )

    args, kwargs = session.post.call_args
    assert args[0] == "http://fake:3001/api/plans/website-v2/tasks"
    body = kwargs["json"]
    assert body["title"] == "Rewrite hero"
    assert body["priority"] == "high"
    assert body["assigned_to"] == "codex"
    assert body["blocked_by"] == ["t-001"]
    assert task.id == "t-005"
    assert task.assigned_to == "codex"


def test_plan_start_posts_body():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_task_body(status="in_progress", started_by="alice"),
    )
    hub = make_hub(session)

    task = hub.plan_start("p1", "t-001", reason="picking up")

    args, kwargs = session.post.call_args
    assert args[0] == "http://fake:3001/api/plans/p1/tasks/t-001/start"
    assert kwargs["json"]["reason"] == "picking up"
    assert task.status == "in_progress"
    assert task.started_by == "alice"


def test_plan_complete_with_proof_object():
    from ctxone import Proof

    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_task_body(
            status="done",
            completed_by="alice",
            proof={"kind": "commit", "value": "abc123", "note": None},
        )
    )
    hub = make_hub(session)

    proof = Proof(kind="commit", value="abc123")
    task = hub.plan_complete("p1", "t-001", proof)

    args, kwargs = session.post.call_args
    assert args[0] == "http://fake:3001/api/plans/p1/tasks/t-001/complete"
    assert kwargs["json"]["proof"] == {"kind": "commit", "value": "abc123"}
    assert task.status == "done"
    assert task.proof.kind == "commit"
    assert task.proof.value == "abc123"


def test_plan_complete_with_proof_dict():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_task_body(
            status="done",
            proof={"kind": "file", "value": "src/foo.rs", "note": "wrote it"},
        )
    )
    hub = make_hub(session)

    hub.plan_complete(
        "p1",
        "t-001",
        {"kind": "file", "value": "src/foo.rs", "note": "wrote it"},
    )

    body = session.post.call_args.kwargs["json"]
    assert body["proof"]["kind"] == "file"
    assert body["proof"]["note"] == "wrote it"


def test_plan_abandon_sends_reason():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_task_body(
            status="abandoned",
            abandoned_reason="superseded",
        )
    )
    hub = make_hub(session)

    task = hub.plan_abandon("p1", "t-001", "superseded")

    body = session.post.call_args.kwargs["json"]
    assert body["reason"] == "superseded"
    assert task.status == "abandoned"
    assert task.abandoned_reason == "superseded"


def test_plan_next_returns_task():
    session = MagicMock()
    session.get.return_value = mock_response(
        json_body={"task": _task_body(id="t-007", priority="critical")}
    )
    hub = make_hub(session, )

    task = hub.plan_next(
        "p1",
        assigned_to="me",
        include_unassigned=True,
        assigned_only=False,
    )

    url = session.get.call_args.args[0]
    assert url == "http://fake:3001/api/plans/p1/next"
    params = session.get.call_args.kwargs["params"]
    assert params["assigned_to"] == "me"
    assert params["include_unassigned"] == "true"
    assert "assigned_only" not in params
    assert task is not None
    assert task.id == "t-007"
    assert task.priority == "critical"


def test_plan_next_returns_none_when_null():
    session = MagicMock()
    session.get.return_value = mock_response(json_body={"task": None})
    hub = make_hub(session)
    assert hub.plan_next("p1") is None


def test_plan_list_filters_by_status():
    session = MagicMock()
    session.get.return_value = mock_response(
        json_body=[
            _plan_body(name="a", status="active"),
            _plan_body(name="b", status="active"),
        ]
    )
    hub = make_hub(session)

    plans = hub.plan_list(status="active")

    args, kwargs = session.get.call_args
    assert args[0] == "http://fake:3001/api/plans"
    assert kwargs["params"]["status"] == "active"
    assert [p.name for p in plans] == ["a", "b"]


def test_plan_get_parses_nested_tasks():
    session = MagicMock()
    session.get.return_value = mock_response(
        json_body=_plan_body(
            name="p1",
            tasks=[_task_body(id="t-001"), _task_body(id="t-002", priority="high")],
            task_counts={
                "pending": 2,
                "in_progress": 0,
                "done": 0,
                "abandoned": 0,
                "total": 2,
            },
        )
    )
    hub = make_hub(session)

    plan = hub.plan_get("p1")
    assert plan.name == "p1"
    assert plan.task_counts.total == 2
    assert len(plan.tasks) == 2
    assert plan.tasks[1].priority == "high"


def test_plan_tasks_returns_list():
    session = MagicMock()
    session.get.return_value = mock_response(
        json_body=[_task_body(id="t-001"), _task_body(id="t-002")]
    )
    hub = make_hub(session)

    tasks = hub.plan_tasks("p1")
    assert [t.id for t in tasks] == ["t-001", "t-002"]


def test_plan_archive_sets_status():
    session = MagicMock()
    session.post.return_value = mock_response(
        json_body=_plan_body(status="archived", archived_at="2026-04-15T20:00:00Z")
    )
    hub = make_hub(session)

    plan = hub.plan_archive("p1")
    assert plan.status == "archived"
    assert plan.archived_at == "2026-04-15T20:00:00Z"


def test_plan_delete_calls_delete():
    session = MagicMock()
    session.delete.return_value = mock_response(json_body={"status": "ok"})
    hub = make_hub(session)

    hub.plan_delete("p1")
    args, kwargs = session.delete.call_args
    assert args[0] == "http://fake:3001/api/plans/p1"
    assert kwargs["params"]["ref"] == "main"


def test_plan_new_sends_agent_header_when_configured():
    session = MagicMock()
    session.post.return_value = mock_response(
        status_code=201, json_body=_plan_body()
    )
    hub = Hub(server="http://fake:3001", session=session, agent_id="claude-code")

    hub.plan_new("p1")

    headers = session.post.call_args.kwargs["headers"]
    assert headers.get("X-CTXone-Agent") == "claude-code"

