# Context Anxiety — AgentStateGraph Memory Product

This directory contains the design, strategy, and implementation plans for the
AgentStateGraph agent memory layer — the product that eliminates "context anxiety."

## The Problem (in one paragraph)

You have 4 AI sessions open, each with different context. You can't close any of
them because the knowledge is trapped. You start a new session and spend 10 minutes
re-explaining everything. Sometimes the new session is great. Sometimes it struggles
with things the old session handled easily. You have no idea why. Meanwhile, every
message costs more tokens because the entire conversation history rides along — most
of it irrelevant to the current question. More context makes the session "smarter"
but it's self-defeating because you're burning tokens and slowing down responses.

## The Solution

AgentStateGraph as a persistent, searchable, accountable memory layer for AI agents.
Every session commits what it learns. Every new session queries for what's relevant.
60x token reduction. Consistent session quality. Transparent agent state.

## Files

- `VISION.md` — The complete product vision
- `USE_CASES.md` — All identified use cases for AgentStateGraph beyond infrastructure ops
- `CONTEXT_ANXIETY.md` — The coined term, marketing language, and pitch angles
- `TOKEN_ECONOMICS.md` — The math on token savings and enterprise ROI
- `MEMORY_MCP_DESIGN.md` — Technical design for the agentstategraph-memory MCP server
