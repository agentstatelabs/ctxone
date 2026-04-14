"""Unit tests for the markdown parser. No Hub required."""

from ctxone.client import _parse_markdown_sections


def test_empty_input():
    assert _parse_markdown_sections("") == []


def test_h1_split():
    md = "# First\n\nbody of first\n\n# Second\n\nbody of second\n"
    sections = _parse_markdown_sections(md)
    assert len(sections) == 2
    assert sections[0]["title"] == "First"
    assert sections[0]["body"] == "body of first"
    assert sections[1]["title"] == "Second"
    assert sections[1]["body"] == "body of second"


def test_h2_split():
    md = "## One\n\ntext one\n\n## Two\n\ntext two"
    sections = _parse_markdown_sections(md)
    assert len(sections) == 2
    assert sections[0]["title"] == "One"
    assert sections[1]["title"] == "Two"


def test_h3_does_not_split():
    md = "# Top\n\nintro\n\n### Sub\n\ndeep content\n"
    sections = _parse_markdown_sections(md)
    assert len(sections) == 1
    assert sections[0]["title"] == "Top"
    assert "intro" in sections[0]["body"]
    assert "### Sub" in sections[0]["body"]
    assert "deep content" in sections[0]["body"]


def test_intro_before_first_heading():
    md = "some preamble\n\nmore preamble\n\n# First\n\nbody\n"
    sections = _parse_markdown_sections(md)
    assert len(sections) == 2
    assert sections[0]["title"] == "Intro"
    assert "preamble" in sections[0]["body"]


def test_empty_body_skipped():
    md = "# Only heading\n"
    sections = _parse_markdown_sections(md)
    assert len(sections) == 0
