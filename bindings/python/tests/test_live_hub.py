"""Integration tests that spin up a real ctxone-hub process.

Skipped automatically if the ctxone-hub binary isn't on PATH or can't
be found at the expected build location. Run with:

    pytest tests/test_live_hub.py -v

To force a specific binary:

    CTXONE_HUB_BIN=/path/to/ctxone-hub pytest tests/test_live_hub.py
"""

from __future__ import annotations

import os
import shutil
import signal
import socket
import subprocess
import tempfile
import time
from pathlib import Path

import pytest

from ctxone import Hub, HubUnreachable


def _find_hub_binary() -> str | None:
    # 1. explicit env var
    if env := os.environ.get("CTXONE_HUB_BIN"):
        if Path(env).exists():
            return env
    # 2. on PATH
    if found := shutil.which("ctxone-hub"):
        return found
    # 3. local debug build (repo layout)
    repo_root = Path(__file__).parents[3]
    candidates = [
        repo_root / "target" / "debug" / "ctxone-hub",
        repo_root / "target" / "release" / "ctxone-hub",
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    return None


HUB_BIN = _find_hub_binary()


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture(scope="module")
def live_hub():
    """Spawn ctxone-hub in HTTP mode with an in-memory storage backend."""
    if HUB_BIN is None:
        pytest.skip("ctxone-hub binary not found; build the workspace first")

    port = _free_port()
    db_file = tempfile.NamedTemporaryFile(suffix=".db", delete=False)
    db_file.close()
    try:
        proc = subprocess.Popen(
            [
                HUB_BIN,
                "--http",
                "--port",
                str(port),
                "--path",
                db_file.name,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        server_url = f"http://127.0.0.1:{port}"
        hub = Hub(server=server_url)

        # Wait up to 5s for the server to accept connections
        deadline = time.time() + 5
        while time.time() < deadline:
            if hub.is_reachable():
                break
            time.sleep(0.1)
        else:
            proc.terminate()
            raise RuntimeError(f"Hub did not start within 5s on {server_url}")

        yield hub

        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    finally:
        Path(db_file.name).unlink(missing_ok=True)


def test_health(live_hub):
    assert live_hub.is_reachable() is True


def test_remember_then_recall(live_hub):
    result = live_hub.remember(
        "Integration test fact about BSL-1.1", context="legal", importance="high"
    )
    assert result.path.startswith("/memory/legal/")
    assert result.commit_id.startswith("sg_")

    recall = live_hub.recall("BSL")
    assert recall.topic_matches >= 1
    values = [r.value for r in recall.results if r.value]
    assert any("BSL-1.1" in v for v in values)
    assert recall.ctx_tokens_sent > 0
    assert recall.ctx_tokens_estimated_flat > 0


def test_prime_and_pinned_grouped(live_hub):
    live_hub.prime(
        "test-vision",
        [
            {"title": "First", "body": "first section body"},
            {"title": "Second", "body": "second section body"},
        ],
        pinned=True,
    )

    grouped = live_hub.pinned_grouped()
    assert "test-vision" in grouped
    titles = {s["title"] for s in grouped["test-vision"]}
    assert "First" in titles
    assert "Second" in titles


def test_pinned_always_in_recall(live_hub):
    # Pin something
    live_hub.prime(
        "must-include",
        [{"title": "Critical", "body": "pinned content"}],
        pinned=True,
    )

    # Recall on an unrelated topic
    result = live_hub.recall("totally-unrelated-xyz")
    assert result.pinned_count >= 1
    has_pinned = any(r.pinned for r in result.results)
    assert has_pinned


def test_ls_and_search(live_hub):
    live_hub.remember("searchable integration content", context="test")

    paths = live_hub.ls("/memory")
    assert len(paths) > 0

    results = live_hub.search("searchable")
    assert len(results) > 0


def test_log_returns_commits(live_hub):
    live_hub.remember("commit for log test", context="test")
    commits = live_hub.log(limit=5)
    assert len(commits) > 0
    assert commits[0].id.startswith("sg_")


def test_branches_create_and_list(live_hub):
    commit = live_hub.create_branch("integration-exp")
    assert commit.startswith("sg_")

    names = [b["name"] for b in live_hub.branches()]
    assert "integration-exp" in names


def test_stats_returns_typed_result(live_hub):
    stats = live_hub.stats()
    assert stats.session_tokens_used >= 0
    assert stats.total_graph_size_chars >= 0


def test_hub_unreachable_error():
    # Point at a port that's almost certainly unused
    bad_hub = Hub(server="http://127.0.0.1:1")
    with pytest.raises(HubUnreachable):
        bad_hub.recall("foo")
