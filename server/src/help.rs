//! On-demand instruction disclosure (`help`) — t-005 of `ctx-context-paradigm`.
//!
//! Feature docs are compiled INTO the binary, so `help` always describes the
//! exact code that is running (version-pinned, immutable, cannot drift). They
//! are served through the `help` MCP tool, `GET /api/help`, and `ctx help` — a
//! single manual for agents and humans alike.
//!
//! Design decisions (persisted under /memory/ctx-context-paradigm):
//!  - Reuse recall's ACCESS PATTERN (topic in -> ranked, budgeted chunk out),
//!    NOT its STORE. Docs never live in the user's memory graph — that would
//!    pollute recall, skew the savings baseline, drift, and mix trust tiers.
//!  - Split the INDEX from the BODY. Each binary compiles its own bodies and
//!    publishes a lightweight manifest (feature -> synopsis -> owner -> version)
//!    to `~/.config/agentstate/help-index.json` so a unified `help` can proxy
//!    across the separate `ctx` and `asd` binaries.

use serde::Serialize;
use serde_json::{Value, json};

/// Which binary owns these docs. Written into the manifest so a unified `help`
/// can route `help <feature>` to the tool that implements it.
pub const OWNER: &str = "ctx";

/// The version of the running binary — stamped into every response so an agent
/// (or a stale manifest) can detect a mismatch against the code it is calling.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone, Serialize)]
pub struct HelpParam {
    pub name: &'static str,
    pub required: bool,
    pub desc: &'static str,
}

/// One feature's compiled documentation. Authored next to the tool it
/// documents; `feature` matches the tool/CLI name so lookups are stable.
#[derive(Debug, Clone, Serialize)]
pub struct HelpDoc {
    pub feature: &'static str,
    /// Catalog grouping for the no-arg `help` overview.
    pub group: &'static str,
    pub synopsis: &'static str,
    pub syntax: &'static str,
    pub params: &'static [HelpParam],
    pub examples: &'static [&'static str],
    pub gotchas: &'static [&'static str],
    pub related: &'static [&'static str],
}

macro_rules! p {
    ($name:literal, $req:literal, $desc:literal) => {
        HelpParam {
            name: $name,
            required: $req,
            desc: $desc,
        }
    };
}

/// The compiled registry. Discoverability-forward: this is the daily-driver
/// hot core, and the no-arg catalog surfaces all of it so agents reach for
/// features they would not otherwise know exist.
pub const REGISTRY: &[HelpDoc] = &[
    HelpDoc {
        feature: "recall",
        group: "memory",
        synopsis: "Load ranked, budget-capped memories for a topic; pinned context always included.",
        syntax: "ctx recall <topic> [--budget <tokens=1500>] [--ref <branch>]",
        params: &[
            p!(
                "topic",
                true,
                "A SPECIFIC domain word or decision word — not 'context'."
            ),
            p!(
                "--budget",
                false,
                "Token budget for the response (default 1500)."
            ),
            p!("--ref", false, "Branch/ref to recall from (default main)."),
        ],
        examples: &[
            "ctx recall authentication",
            "ctx recall \"database schema\" --budget 800",
        ],
        gotchas: &[
            "Every response carries ctx_savings_ratio; below 2x means the topic was too broad — narrow it.",
            "To find a value whose path you don't know use `search`; to fetch a known path use `get`.",
        ],
        related: &["remember", "search", "context", "prime"],
    },
    HelpDoc {
        feature: "remember",
        group: "memory",
        synopsis: "Store a durable fact, decision, or preference that survives sessions and tool switches.",
        syntax: "ctx remember <fact> [--importance low|medium|high] [--context <ns>] [--tags a,b]",
        params: &[
            p!("fact", true, "The fact/decision text to store."),
            p!(
                "--importance",
                false,
                "high=explicit decision/policy, medium=default, low=trivia."
            ),
            p!(
                "--context",
                false,
                "Namespace/project to file the memory under."
            ),
        ],
        examples: &["ctx remember \"BSL-1.1 for new repos\" --importance high --context licensing"],
        gotchas: &[
            "To import a whole markdown doc's sections at once use `prime`, not repeated remember calls.",
            "Don't ask permission — if the user said it, it's worth remembering.",
        ],
        related: &["recall", "prime", "forget"],
    },
    HelpDoc {
        feature: "prime",
        group: "memory",
        synopsis: "Import a markdown doc's sections as searchable memories so agents recall it without re-reading the file.",
        syntax: "ctx prime <file> [--source <name>] [--pin]",
        params: &[
            p!("file", true, "Path to the markdown doc to import."),
            p!(
                "--source",
                false,
                "Reuse the same source name to re-import updated content idempotently."
            ),
            p!(
                "--pin",
                false,
                "Pin every section so it's always included in recall (critical context only)."
            ),
        ],
        examples: &["ctx prime ARCHITECTURE.md --source arch --pin"],
        gotchas: &[
            "Keep the file canonical in the repo; import rationale/summary rather than pasting a whole doc that drifts.",
            "To register a POINTER to a doc (path/status) instead of its content, use `docs_find`.",
        ],
        related: &["recall", "remember", "docs_find"],
    },
    HelpDoc {
        feature: "context",
        group: "memory",
        synopsis: "Load the full stored context for a named project — unranked, unbudgeted.",
        syntax: "ctx context <project>",
        params: &[p!(
            "project",
            true,
            "Project whose whole memory subtree to load."
        )],
        examples: &["ctx context ctxone"],
        gotchas: &["For a budgeted, topic-scoped slice use `recall` instead."],
        related: &["recall", "search"],
    },
    HelpDoc {
        feature: "search",
        group: "memory",
        synopsis: "Literal substring scan over the memory graph — no ranking, no budget.",
        syntax: "ctx search <query> [--max <n>]",
        params: &[
            p!(
                "query",
                true,
                "Literal substring to match against stored values."
            ),
            p!("--max", false, "Cap the number of matches returned."),
        ],
        examples: &["ctx search glpat"],
        gotchas: &[
            "Unlike recall this is not LLM-oriented — it returns full matching paths/values.",
        ],
        related: &["recall", "ls", "get"],
    },
    HelpDoc {
        feature: "forget",
        group: "memory",
        synopsis: "Delete a memory at a specific path.",
        syntax: "ctx forget <path>",
        params: &[p!(
            "path",
            true,
            "Exact memory path to remove (from search/ls)."
        )],
        examples: &["ctx forget /memory/setup/18a9cb876ca58e40"],
        gotchas: &["Deletion is durable; confirm the path with `search`/`ls` first."],
        related: &["search", "ls"],
    },
    HelpDoc {
        feature: "what_changed_since",
        group: "memory",
        synopsis: "See memory-graph commits since a date, with intent category and confidence.",
        syntax: "ctx what-changed-since <iso-8601>",
        params: &[p!(
            "since",
            true,
            "ISO-8601 timestamp; use the date from the user's prompt."
        )],
        examples: &["ctx what-changed-since 2026-07-26T00:00:00Z"],
        gotchas: &[
            "Auto-capture (session ingest) turns dominate the log at confidence 0.00 — filter for real decisions.",
        ],
        related: &["why_did_we", "recall"],
    },
    HelpDoc {
        feature: "summarize_session",
        group: "memory",
        synopsis: "End-of-session knowledge commit: one cohesive snapshot of key points + decisions a future session can recall as a unit.",
        syntax: "ctx summarize-session --key-points <...> [--decisions <...>]",
        params: &[
            p!("key_points", true, "Observed facts from the session."),
            p!("decisions", false, "Choices made during the session."),
        ],
        examples: &["ctx summarize-session --key-points \"chose manifest federation for help\""],
        gotchas: &["Call only for real working sessions, not quick Q&A."],
        related: &["remember", "recall"],
    },
    HelpDoc {
        feature: "plan_new",
        group: "plans",
        synopsis: "Create a plan to track multi-step work across sessions.",
        syntax: "ctx plan new <name> [--description <text>]",
        params: &[p!("name", true, "kebab-case plan name, used in paths.")],
        examples: &["ctx plan new website-v2"],
        gotchas: &["Fails if a plan with that name already exists on the branch."],
        related: &["plan_add", "plan_start", "plan_next"],
    },
    HelpDoc {
        feature: "plan_add",
        group: "plans",
        synopsis: "Add a task to a plan; optionally assign it to a specific agent.",
        syntax: "ctx plan add <plan> <title> [--priority ...] [--assigned-to <agent>] [--blocked-by <ids>]",
        params: &[
            p!("plan_id", true, "Plan to add the task to."),
            p!("title", true, "Imperative one-line task title."),
            p!("--priority", false, "critical|high|medium|low."),
        ],
        examples: &["ctx plan add website-v2 \"Wire the nav\" --priority high"],
        gotchas: &[
            "Blockers must already exist in the plan; a plan may auto-lock past a done ratio (use --force).",
        ],
        related: &["plan_new", "plan_start", "plan_complete"],
    },
    HelpDoc {
        feature: "plan_start",
        group: "plans",
        synopsis: "Move a task from pending to in_progress; refuses if blockers aren't done.",
        syntax: "ctx plan start <plan> <task_id>",
        params: &[
            p!("plan_id", true, "Plan holding the task."),
            p!("task_id", true, "Task to start (e.g. t-003)."),
        ],
        examples: &["ctx plan start ctx-context-paradigm t-005"],
        gotchas: &[
            "Warns (doesn't block) if other tasks are already in_progress — finish or abandon stale ones.",
        ],
        related: &["plan_add", "plan_complete", "plan_next"],
    },
    HelpDoc {
        feature: "plan_complete",
        group: "plans",
        synopsis: "Force-complete a WHOLE plan: abandon every still-open task with a reason, then promote the plan to completed.",
        syntax: "plan_complete {plan_id, [reason]}  (MCP tool)",
        params: &[
            p!("plan_id", true, "Plan to force-complete."),
            p!(
                "reason",
                false,
                "Reason stamped on each auto-abandoned task."
            ),
        ],
        examples: &["plan_complete {plan_id: \"website-v2\", reason: \"scope cut\"}"],
        gotchas: &[
            "To complete a single TASK with proof use `plan_done`, not this — this closes the whole plan.",
            "Idempotent on already-completed plans; refuses on archived or empty plans.",
        ],
        related: &["plan_done", "plan_archive", "plan_abandon"],
    },
    HelpDoc {
        feature: "plan_next",
        group: "plans",
        synopsis: "Get the highest-priority pending task whose blockers are done; supports assigned_to='me'.",
        syntax: "ctx plan next [--plan <name>] [--assigned-to me]",
        params: &[p!(
            "--assigned-to",
            false,
            "'me' maps to the caller's agent id for multi-agent orchestration."
        )],
        examples: &["ctx plan next --plan ctx-context-paradigm"],
        gotchas: &[
            "The plan IS the orchestration layer — agents coordinate through shared task state.",
        ],
        related: &["plan_start", "plan_list"],
    },
    HelpDoc {
        feature: "plan_list",
        group: "plans",
        synopsis: "List active plans and their task counts — call at session start to see what's in flight.",
        syntax: "ctx plan list",
        params: &[],
        examples: &["ctx plan list"],
        gotchas: &["For the full task tree of one plan use `plan_show`, not list + N calls."],
        related: &["plan_show", "plan_next"],
    },
    HelpDoc {
        feature: "why_did_we",
        group: "provenance",
        synopsis: "Recall the rationale behind a past decision.",
        syntax: "ctx why-did-we <topic>",
        params: &[p!("topic", true, "Decision/topic to explain.")],
        examples: &["ctx why-did-we \"help docs not in memory\""],
        gotchas: &[
            "Answers from stored decisions; if none exist, capture one with `remember` first.",
        ],
        related: &["what_changed_since", "recall"],
    },
    HelpDoc {
        feature: "help",
        group: "meta",
        synopsis: "Get exact syntax, examples, and gotchas for a feature on demand — call before using a feature you're unsure of.",
        syntax: "ctx help [topic]",
        params: &[p!(
            "topic",
            false,
            "Feature name or a phrase; omit to list the whole catalog."
        )],
        examples: &[
            "ctx help",
            "ctx help remember",
            "ctx help \"save a memory\"",
        ],
        gotchas: &[
            "Docs are version-pinned to the running binary, so they can't drift from the code.",
        ],
        related: &["recall"],
    },
    // ===== memory =====
    HelpDoc {
        feature: "get",
        group: "memory",
        synopsis: "Read the raw JSON value stored at a known path (string, object, list) — not keyword-ranked.",
        syntax: "ctx get <path>",
        params: &[p!(
            "path",
            true,
            "Exact memory path (from `ls` or `search`)."
        )],
        examples: &["ctx get /memory/facts/abc123"],
        gotchas: &["For free-text recall use `recall`; `get` needs the exact path."],
        related: &["ls", "search", "recall"],
    },
    HelpDoc {
        feature: "ls",
        group: "memory",
        synopsis: "List every path under a prefix on a branch — cheap discovery of what's actually stored.",
        syntax: "ctx ls [prefix=/] [--max-depth <n=50>]",
        params: &[
            p!(
                "prefix",
                false,
                "Subtree to walk (e.g. /memory/primed, /plans, /sessions)."
            ),
            p!(
                "--max-depth",
                false,
                "Limit walk depth from the prefix; default 50."
            ),
        ],
        examples: &["ctx ls /memory/primed", "ctx ls /plans --max-depth 2"],
        gotchas: &["Returns leaf paths — enumerate before guessing path names."],
        related: &["get", "search", "blame"],
    },
    HelpDoc {
        feature: "blame",
        group: "memory",
        synopsis: "Full provenance chain for a path: every commit that touched it, who wrote it, intent + confidence.",
        syntax: "ctx blame <path>",
        params: &[p!(
            "path",
            true,
            "Memory path to trace (from `ls`/`search`)."
        )],
        examples: &["ctx blame /memory/facts/licensing"],
        gotchas: &[
            "Call BEFORE trusting a stored value on high-stakes topics (security, licensing, deploy).",
            "Use `why_did_we` when you have a decision phrase, not a path.",
        ],
        related: &["why_did_we", "log", "get"],
    },
    HelpDoc {
        feature: "docs_find",
        group: "memory",
        synopsis: "Search the canonical-doc registry (path/scope/answers/owner); returns POINTERS to .md docs, not content.",
        syntax: "ctx docs find [query]",
        params: &[p!(
            "query",
            false,
            "Substring over path/scope/answers/owner; omit to list all."
        )],
        examples: &["ctx docs find licensing", "ctx docs find"],
        gotchas: &[
            "Distinct from `prime`, which imports a doc's CONTENT into recall; this returns a pointer to the file.",
        ],
        related: &["prime", "recall", "docs"],
    },
    // ===== plans =====
    HelpDoc {
        feature: "plan_done",
        group: "plans",
        synopsis: "Transition a task to `done` with a required proof (commit>file>test>text); auto-completes the plan if last open.",
        syntax: "ctx plan done <plan> <task_id> --proof <kind:value[:note]> [--reason <text>]",
        params: &[
            p!("plan_id", true, "Plan holding the task."),
            p!("task_id", true, "Task to complete (e.g. t-003)."),
            p!(
                "--proof",
                true,
                "kind:value[:note] where kind is commit|file|test|text."
            ),
        ],
        examples: &["ctx plan done ctx-context-paradigm t-005 --proof commit:abc1234"],
        gotchas: &["Proof is stored but NOT verified at call time; prefer a commit SHA."],
        related: &["plan_start", "plan_abandon", "plan_complete"],
    },
    HelpDoc {
        feature: "plan_abandon",
        group: "plans",
        synopsis: "Mark a task `abandoned` with a required reason — a first-class outcome recorded in blame, not deletion.",
        syntax: "ctx plan abandon <plan> <task_id> --reason <text>",
        params: &[
            p!("plan_id", true, "Plan holding the task."),
            p!("task_id", true, "Task to abandon."),
            p!("--reason", true, "Why it's abandoned; recorded in blame."),
        ],
        examples: &["ctx plan abandon website-v2 t-004 --reason \"superseded by t-009\""],
        gotchas: &[
            "Legal from pending or in_progress; abandoning the last open task auto-completes the plan.",
        ],
        related: &["plan_done", "plan_complete", "plan_start"],
    },
    HelpDoc {
        feature: "plan_show",
        group: "plans",
        synopsis: "Fetch one plan with full task list, statuses, proofs, and per-task assignment.",
        syntax: "ctx plan show <plan>",
        params: &[p!("plan_id", true, "Plan to display.")],
        examples: &["ctx plan show ctx-context-paradigm"],
        gotchas: &[
            "Cheaper than `plan_list` + N `plan_tasks` calls; use `plan_tasks` for the flat list only.",
        ],
        related: &["plan_tasks", "plan_list", "plan_next"],
    },
    HelpDoc {
        feature: "plan_tasks",
        group: "plans",
        synopsis: "List every task in a plan (flat, with assigned_to) — no plan-envelope metadata.",
        syntax: "ctx plan tasks <plan>",
        params: &[p!("plan_id", true, "Plan whose tasks to list.")],
        examples: &["ctx plan tasks website-v2"],
        gotchas: &[
            "`plan_show` returns the same tasks plus the plan envelope if you also need that.",
        ],
        related: &["plan_show", "plan_list"],
    },
    HelpDoc {
        feature: "plan_link",
        group: "plans",
        synopsis: "Advisory cross-plan dependency: mark that a task, when done, satisfies a task in ANOTHER plan.",
        syntax: "ctx plan link <plan> <task_id> <target>",
        params: &[
            p!("plan_id", true, "Plan holding the satisfying task."),
            p!("task_id", true, "Task doing the satisfying (e.g. t-003)."),
            p!(
                "target",
                true,
                "Target it satisfies, as `plan/task` (e.g. other-plan/t-002)."
            ),
        ],
        examples: &["ctx plan link routing t-003 foundation/t-002"],
        gotchas: &[
            "Advisory only — does not auto-close the target; completing this task surfaces a reminder.",
        ],
        related: &["plan_done", "plan_show"],
    },
    HelpDoc {
        feature: "plan_stale",
        group: "plans",
        synopsis: "List in-progress tasks with no progress in N days (default 7), most-stale first, across active plans.",
        syntax: "ctx plan stale [--days <n=7>] [--all-namespaces]",
        params: &[p!(
            "--days",
            false,
            "Staleness threshold in days (default 7)."
        )],
        examples: &["ctx plan stale --days 14"],
        gotchas: &[
            "Complements `plan_next`, which only shows the next PENDING task and never what's in-progress.",
        ],
        related: &["plan_next", "plan_show"],
    },
    HelpDoc {
        feature: "plan_archive",
        group: "plans",
        synopsis: "Soft, reversible archive of a plan — sets status `archived`, stamps archived_at, preserves task data.",
        syntax: "ctx plan archive <plan>",
        params: &[p!("plan_id", true, "Plan to archive.")],
        examples: &["ctx plan archive website-v1"],
        gotchas: &[
            "Reversible and keeps history browsable; use `plan_complete` to close open tasks instead.",
        ],
        related: &["plan_complete", "plan_move", "plan_list"],
    },
    HelpDoc {
        feature: "plan_move",
        group: "plans",
        synopsis: "Move a plan (with its tasks and links) to another workspace/namespace — for plans created in the wrong one.",
        syntax: "ctx plan relocate <plan> --to <workspace> [--namespace <src>]",
        params: &[
            p!("plan_id", true, "Plan to relocate."),
            p!("--to", true, "Destination workspace/namespace."),
            p!(
                "--namespace",
                false,
                "Source namespace if the plan isn't in the resolved one."
            ),
        ],
        examples: &["ctx plan relocate website-v2 --to ctxone"],
        gotchas: &[
            "Fixes plans that landed in `default` before per-workspace routing; reads from the current namespace.",
        ],
        related: &["plan_show", "plan_list", "project_status"],
    },
    // ===== branches =====
    HelpDoc {
        feature: "branches",
        group: "branches",
        synopsis: "List every branch in the graph with its current head commit id.",
        syntax: "ctx branches",
        params: &[],
        examples: &["ctx branches"],
        gotchas: &["Branch names are free-form — check before assuming `feature/x` exists."],
        related: &["branch", "log", "diff"],
    },
    HelpDoc {
        feature: "branch",
        group: "branches",
        synopsis: "Create a new branch starting from `from` (default main) — cheap; prefer a branch over racing writes on main.",
        syntax: "ctx branch create <name> [--from <ref=main>]",
        params: &[
            p!("name", true, "Name of the new branch."),
            p!(
                "--from",
                false,
                "Branch/tag/commit to start from (default main)."
            ),
        ],
        examples: &["ctx branch create feat/idea --from main"],
        gotchas: &[
            "Use a branch to stage memory writes that shouldn't land on main yet, then `merge`.",
        ],
        related: &["branches", "merge", "diff"],
    },
    HelpDoc {
        feature: "merge",
        group: "branches",
        synopsis: "Merge a source branch into a target (default main); returns the new commit or a structured conflict list.",
        syntax: "ctx merge <source> [--into <target=main>] [-m <msg>] [--dry-run] [--allow-deletions] [--allow-regressions]",
        params: &[
            p!("source", true, "Branch with new changes."),
            p!(
                "--into",
                false,
                "Target branch to merge into (default main)."
            ),
            p!(
                "--dry-run",
                false,
                "Preview added/changed/removed + conflicts without writing."
            ),
            p!(
                "--allow-regressions",
                false,
                "Permit moving a completed task back to non-terminal."
            ),
        ],
        examples: &[
            "ctx merge feat/idea --dry-run",
            "ctx merge feat/idea -m \"land idea\"",
        ],
        gotchas: &[
            "Blocks by default if the merge would delete entries (--allow-deletions) or regress a plan task.",
            "Resolve conflicts by writing the desired value on the target, then re-run.",
        ],
        related: &["diff", "branch", "branches"],
    },
    HelpDoc {
        feature: "diff",
        group: "branches",
        synopsis: "Structural diff between two refs — the set/delete ops to turn ref_a into ref_b (not a textual diff).",
        syntax: "ctx diff <ref_a> <ref_b>",
        params: &[
            p!("ref_a", true, "First ref (usually older/base)."),
            p!("ref_b", true, "Second ref (usually newer/target)."),
        ],
        examples: &["ctx diff main feat/idea"],
        gotchas: &["Pair with `branches` to find ref names; inspect before merging."],
        related: &["merge", "branches", "log"],
    },
    HelpDoc {
        feature: "log",
        group: "branches",
        synopsis: "Last N commits on a branch (newest first): agent id, intent category, description, confidence, tags.",
        syntax: "ctx log [-n <limit=20>]",
        params: &[p!("-n", false, "Max commits to show (default 20).")],
        examples: &["ctx log -n 50"],
        gotchas: &[
            "Broader than `blame` (per-path) and cheaper than `what_changed_since` for an absolute count.",
        ],
        related: &["blame", "what_changed_since", "branches"],
    },
    // ===== code (proxy to ASD; MCP-first, no dedicated `ctx` subcommand) =====
    HelpDoc {
        feature: "code_repos",
        group: "code",
        synopsis: "List every ASD code repo registered with this hub as [{name, url}] — the names feed every code tool's `repo` param.",
        syntax: "code_repos {}  (MCP tool — no CLI subcommand)",
        params: &[],
        examples: &["code_repos {}"],
        gotchas: &[
            "Call first when you don't know repo names; skippable when only one repo is registered (code tools default to it).",
        ],
        related: &["code_search", "code_read", "code_impact"],
    },
    HelpDoc {
        feature: "code_search",
        group: "code",
        synopsis: "Rank CODE symbols by concept/keyword across name, signature, doc, path, and ledger in an ASD-indexed repo.",
        syntax: "code_search {query, [repo], [kind], [language], [limit]}  (MCP tool)",
        params: &[
            p!(
                "query",
                true,
                "Concept or keyword to search source symbols for."
            ),
            p!(
                "repo",
                false,
                "Repo name from `code_repos`; optional when only one is registered."
            ),
            p!(
                "kind",
                false,
                "Narrow by symbol kind (e.g. function, struct)."
            ),
        ],
        examples: &["code_search {query: \"merge conflict\"}"],
        gotchas: &[
            "Searches SOURCE CODE — distinct from `search` (memory substrings) and `recall` (memory facts).",
        ],
        related: &["code_read", "callers_of", "callees_of", "code_repos"],
    },
    HelpDoc {
        feature: "code_read",
        group: "code",
        synopsis: "Read one CODE symbol by qname, returning {symbol, effects, ledger}: signature, doc, effects, decisions.",
        syntax: "code_read {qname, [repo]}  (MCP tool)",
        params: &[
            p!(
                "qname",
                true,
                "Fully-qualified symbol name (from code_search/callers_of/callees_of)."
            ),
            p!(
                "repo",
                false,
                "Repo name; optional when only one is registered."
            ),
        ],
        examples: &["code_read {qname: \"server::merge\"}"],
        gotchas: &["Reads SOURCE CODE, not memory — to fetch a value at a memory path use `get`."],
        related: &["code_search", "callers_of", "callees_of"],
    },
    HelpDoc {
        feature: "callers_of",
        group: "code",
        synopsis: "List the symbols that call a given symbol (inbound edges) in an ASD-indexed repo — its in-repo blast radius.",
        syntax: "callers_of {qname, [repo]}  (MCP tool)",
        params: &[
            p!("qname", true, "Symbol whose callers to list."),
            p!(
                "repo",
                false,
                "Repo name; optional when only one is registered."
            ),
        ],
        examples: &["callers_of {qname: \"server::merge\"}"],
        gotchas: &[
            "In-repo only; for cross-repo consumers use `code_impact`. Inverse is `callees_of`.",
        ],
        related: &["callees_of", "code_impact", "code_read"],
    },
    HelpDoc {
        feature: "callees_of",
        group: "code",
        synopsis: "List the symbols a given symbol calls (outbound edges) in an ASD-indexed repo — how it's implemented.",
        syntax: "callees_of {qname, [repo]}  (MCP tool)",
        params: &[
            p!("qname", true, "Symbol whose callees to list."),
            p!(
                "repo",
                false,
                "Repo name; optional when only one is registered."
            ),
        ],
        examples: &["callees_of {qname: \"server::merge\"}"],
        gotchas: &["The inverse of `callers_of`."],
        related: &["callers_of", "code_read", "code_search"],
    },
    HelpDoc {
        feature: "code_impact",
        group: "code",
        synopsis: "Decision-aware federated impact: downstream consumers of an endpoint in OTHER repos + invariants they carry.",
        syntax: "code_impact {target}  (MCP tool)",
        params: &[p!(
            "target",
            true,
            "Route-handler qname (e.g. get_orders) or contract (http:GET /api/orders/{})."
        )],
        examples: &["code_impact {target: \"http:GET /api/orders/{}\"}"],
        gotchas: &[
            "Answers \"what breaks if I change this, and what did those callers promise?\" Index consumer repos first.",
        ],
        related: &["code_cross_repo_edges", "callers_of", "code_repos"],
    },
    HelpDoc {
        feature: "code_cross_repo_edges",
        group: "code",
        synopsis: "Map ALL cross-repo service edges: a client call in one registered repo matched to the route serving it in another.",
        syntax: "code_cross_repo_edges {}  (MCP tool)",
        params: &[],
        examples: &["code_cross_repo_edges {}"],
        gotchas: &[
            "Returns every edge (federated view); for one endpoint's blast radius use `code_impact`. Index repos first.",
        ],
        related: &["code_impact", "code_repos"],
    },
    // ===== reminders (MCP-first; CLI only exposes `ctx reminder tick`) =====
    HelpDoc {
        feature: "reminder_create",
        group: "reminders",
        synopsis: "Schedule a pull-based reminder; retrieve later via `remind_me`. Defaults to needing approval before it acts.",
        syntax: "reminder_create {text, due_at, [autonomous], [schedule], [priority]}  (MCP tool)",
        params: &[
            p!("text", true, "What to be reminded about."),
            p!(
                "autonomous",
                false,
                "false (default) => awaiting_permission until `reminder_approve`; true runs unattended."
            ),
            p!(
                "schedule",
                false,
                "kind: interval|daily|weekly to re-fire; omit for one-shot."
            ),
        ],
        examples: &[
            "reminder_create {text: \"weekly metrics review\", schedule: {kind: \"weekly\"}}",
        ],
        gotchas: &[
            "Fail-closed: non-autonomous reminders must be approved before anything acts on them.",
        ],
        related: &["remind_me", "reminder_approve", "reminder_list"],
    },
    HelpDoc {
        feature: "remind_me",
        group: "reminders",
        synopsis: "Return all currently-actionable reminders (due or awaiting_permission), priority-ordered; lazily promotes overdue ones.",
        syntax: "remind_me {}  (MCP tool)",
        params: &[],
        examples: &["remind_me {}"],
        gotchas: &[
            "Primary reminder surface — call at session start; awaiting_permission items need `reminder_approve` first.",
        ],
        related: &["reminder_create", "reminder_approve", "reminder_list"],
    },
    HelpDoc {
        feature: "reminder_list",
        group: "reminders",
        synopsis: "Browse reminders with optional filters (status/priority/tag/ref), ordered by priority then due_at.",
        syntax: "reminder_list {[status], [priority], [tag], [ref]}  (MCP tool)",
        params: &[p!("status", false, "Filter by reminder status.")],
        examples: &["reminder_list {status: \"pending\"}"],
        gotchas: &[
            "For actionable items prefer `remind_me` — it handles lazy promotion automatically.",
        ],
        related: &["remind_me", "reminder_get", "reminder_create"],
    },
    HelpDoc {
        feature: "reminder_get",
        group: "reminders",
        synopsis: "Fetch a single reminder by id with its full execution history (schedule, status, autonomy, past attempts).",
        syntax: "reminder_get {id}  (MCP tool)",
        params: &[p!(
            "id",
            true,
            "Reminder id (from remind_me/reminder_list/reminder_create)."
        )],
        examples: &["reminder_get {id: \"rem-abc123\"}"],
        gotchas: &[
            "To discover actionable reminders use `remind_me`; to browse by status/tag use `reminder_list`.",
        ],
        related: &["reminder_list", "remind_me"],
    },
    HelpDoc {
        feature: "reminder_approve",
        group: "reminders",
        synopsis: "Approve a non-autonomous reminder for execution: awaiting_permission => due.",
        syntax: "reminder_approve {id, [approved_by]}  (MCP tool)",
        params: &[
            p!("id", true, "Reminder to approve."),
            p!(
                "approved_by",
                false,
                "Approver id; defaults to the session agent."
            ),
        ],
        examples: &["reminder_approve {id: \"rem-abc123\"}"],
        gotchas: &[
            "Only after the user explicitly okays it; then call `remind_me` or `reminder_start`.",
        ],
        related: &["remind_me", "reminder_start", "reminder_create"],
    },
    HelpDoc {
        feature: "reminder_snooze",
        group: "reminders",
        synopsis: "Snooze a reminder until a later time; it returns to `due` and reappears on the next `remind_me`.",
        syntax: "reminder_snooze {id, until}  (MCP tool)",
        params: &[
            p!("id", true, "Reminder to snooze."),
            p!("until", true, "Datetime to snooze until (RFC3339)."),
        ],
        examples: &["reminder_snooze {id: \"rem-abc123\", until: \"2026-08-01T09:00:00Z\"}"],
        gotchas: &["Defers rather than cancels — use when waiting on a PR/deploy/the user."],
        related: &["reminder_cancel", "remind_me"],
    },
    HelpDoc {
        feature: "reminder_cancel",
        group: "reminders",
        synopsis: "Cancel a reminder permanently so it never fires again.",
        syntax: "reminder_cancel {id}  (MCP tool)",
        params: &[p!("id", true, "Reminder to cancel.")],
        examples: &["reminder_cancel {id: \"rem-abc123\"}"],
        gotchas: &["Permanent — use `reminder_snooze` to defer instead."],
        related: &["reminder_snooze", "reminder_list"],
    },
    HelpDoc {
        feature: "reminder_start",
        group: "reminders",
        synopsis: "Mark a due reminder in-progress, opening a partial execution record (stamps start time + agent).",
        syntax: "reminder_start {id, [agent_id]}  (MCP tool)",
        params: &[
            p!("id", true, "Reminder to start."),
            p!(
                "agent_id",
                false,
                "Executing agent; defaults to the session agent."
            ),
        ],
        examples: &["reminder_start {id: \"rem-abc123\"}"],
        gotchas: &["Follow with `reminder_record` when you finish — that closes the record."],
        related: &["reminder_record", "remind_me", "reminder_approve"],
    },
    HelpDoc {
        feature: "reminder_record",
        group: "reminders",
        synopsis: "Record the outcome of a reminder execution, closing the record opened by `reminder_start`.",
        syntax: "reminder_record {id, result}  (MCP tool)",
        params: &[
            p!("id", true, "Reminder being recorded."),
            p!("result", true, "success|failed|deferred|snoozed|cancelled."),
        ],
        examples: &["reminder_record {id: \"rem-abc123\", result: \"success\"}"],
        gotchas: &[
            "Call after EVERY attempt — the history is the audit trail.",
            "On success a repeating reminder resets to pending; failed/deferred returns it to due.",
        ],
        related: &["reminder_start", "remind_me"],
    },
    // ===== taint =====
    HelpDoc {
        feature: "taint_list",
        group: "taint",
        synopsis: "List active taints/quarantines/watches; filter by path prefix, kind, or include resolved history.",
        syntax: "ctx taint list [--path-prefix <p>] [--kind taint|quarantine|watch] [--include-resolved]",
        params: &[
            p!(
                "--path-prefix",
                false,
                "Scope to taints whose path starts with this prefix."
            ),
            p!("--kind", false, "taint|quarantine|watch."),
            p!(
                "--include-resolved",
                false,
                "Also show resolved (untainted) entries."
            ),
        ],
        examples: &["ctx taint list --kind quarantine"],
        gotchas: &[
            "Inspect before writing into a sensitive subtree; use the returned id with `taint_remove`.",
        ],
        related: &["taint_check", "taint_apply", "taint_remove"],
    },
    HelpDoc {
        feature: "taint_check",
        group: "taint",
        synopsis: "Read-only: can `agent_id` write to `path` at `confidence` given active taints? Returns can_write + blocking effect.",
        syntax: "ctx taint check <path> [--as <agent>] [--confidence <f=1.0>]",
        params: &[
            p!("path", true, "Path to test a hypothetical write against."),
            p!(
                "--as",
                false,
                "Agent attempting the write; defaults to session agent."
            ),
            p!(
                "--confidence",
                false,
                "Confidence of the proposed write (default 1.0)."
            ),
        ],
        examples: &["ctx taint check /memory/secrets --confidence 0.5"],
        gotchas: &["Cheaper than failing the write and parsing the error; does not modify state."],
        related: &["taint_list", "taint_apply", "taint_remove"],
    },
    HelpDoc {
        feature: "taint_apply",
        group: "taint",
        synopsis: "Apply a taint, quarantine, or watch to a path — guard writes into bad/untrusted/review-needed subtrees.",
        syntax: "ctx taint apply <path> --name <n> [--kind taint|quarantine|watch] [--effect ...] [--severity ...] --reason <r> [--authorized a,b]",
        params: &[
            p!(
                "path",
                true,
                "Path to guard (prefix-matched by later write checks)."
            ),
            p!("--name", true, "Human-readable name for this taint."),
            p!(
                "--effect",
                false,
                "taint kind only: warn|block|review|isolate|advisory (required for kind=taint)."
            ),
            p!(
                "--reason",
                true,
                "Why it's being tainted (recorded for audit)."
            ),
        ],
        examples: &[
            "ctx taint apply /memory/imported --name untrusted --effect block --reason \"unvetted source\"",
        ],
        gotchas: &[
            "block/review stop writes; warn/advisory log; isolate/quarantine confine to authorized agents.",
        ],
        related: &["taint_remove", "taint_check", "taint_list"],
    },
    HelpDoc {
        feature: "taint_remove",
        group: "taint",
        synopsis: "Resolve (lift) an active taint/quarantine/watch by id — marked resolved with a reason, not deleted.",
        syntax: "ctx taint remove <taint_id> --reason <text>",
        params: &[
            p!(
                "taint_id",
                true,
                "Taint id to resolve (find via `taint list`)."
            ),
            p!(
                "--reason",
                true,
                "Why it's being resolved (recorded for audit)."
            ),
        ],
        examples: &["ctx taint remove tnt-abc123 --reason \"source vetted\""],
        gotchas: &["The taint is retained as resolved for audit; get the id from `taint_list`."],
        related: &["taint_list", "taint_apply", "taint_check"],
    },
    // ===== project (session/repo context; MCP-first) =====
    HelpDoc {
        feature: "project_status",
        group: "project",
        synopsis: "Show which namespace this session's writes land in, plus the agent id stamped on commits.",
        syntax: "project_status {}  (MCP tool)",
        params: &[],
        examples: &["project_status {}"],
        gotchas: &[
            "Use to prove where a write went, or debug missing memories (usually a different namespace).",
        ],
        related: &["session_info", "get_active_repo"],
    },
    HelpDoc {
        feature: "get_active_repo",
        group: "project",
        synopsis: "Return the active ASD repo for this session (or null) plus the list of registered repo names.",
        syntax: "get_active_repo {}  (MCP tool)",
        params: &[],
        examples: &["get_active_repo {}"],
        gotchas: &["The active repo is what code tools default to when `repo` is omitted."],
        related: &["set_active_repo", "code_repos"],
    },
    HelpDoc {
        feature: "set_active_repo",
        group: "project",
        synopsis: "Set the session's active ASD repo so code tools default to it when `repo` is omitted; empty string clears.",
        syntax: "set_active_repo {repo}  (MCP tool)",
        params: &[p!(
            "repo",
            true,
            "Registered repo name; empty string clears the active repo."
        )],
        examples: &["set_active_repo {repo: \"ctxone\"}"],
        gotchas: &[
            "Errors if the repo isn't registered — list names with `code_repos`/`get_active_repo`.",
        ],
        related: &["get_active_repo", "code_repos", "code_search"],
    },
    HelpDoc {
        feature: "session_info",
        group: "project",
        synopsis: "Describe a session: its workspace/namespace, branches it touched (turn counts), and plan tasks it worked on.",
        syntax: "session_info {session_id}  (MCP tool)",
        params: &[p!("session_id", true, "Session to describe.")],
        examples: &["session_info {session_id: \"sess-abc123\"}"],
        gotchas: &[
            "If it reports the wrong workspace you're on the wrong namespace (reconnect ?namespace=, or set CTX_NAMESPACE).",
        ],
        related: &["session_link_plan", "project_status"],
    },
    HelpDoc {
        feature: "session_link_plan",
        group: "project",
        synopsis: "Associate a session with a plan task it advanced but didn't name in commit messages; validated against local plans.",
        syntax: "session_link_plan {session_id, plan, task}  (MCP tool)",
        params: &[
            p!("session_id", true, "Session to link."),
            p!("plan", true, "Plan name the task belongs to."),
            p!("task", true, "Task ref, `t-4` or `t-004`."),
        ],
        examples: &[
            "session_link_plan {session_id: \"sess-abc\", plan: \"website-v2\", task: \"t-004\"}",
        ],
        gotchas: &[
            "Commit-message links are derived automatically; this records the rest. validated=false often means wrong workspace.",
        ],
        related: &["session_info", "plan_show"],
    },
    // ===== stats =====
    HelpDoc {
        feature: "record_llm_usage",
        group: "stats",
        synopsis: "Report real LLM token usage (from the model's `usage` field) for ground-truth cost/cache metrics in Lens.",
        syntax: "ctx record-usage --input <n> --output <n> [--cache-read <n>] [--cache-create <n>] [--model <id>] [--provider <id>]",
        params: &[
            p!("--input", true, "Input/prompt tokens consumed."),
            p!("--output", true, "Output/completion tokens generated."),
            p!(
                "--cache-read",
                false,
                "Cache-hit (read) tokens (default 0)."
            ),
            p!(
                "--cache-create",
                false,
                "Cache-creation tokens (default 0)."
            ),
        ],
        examples: &[
            "ctx record-usage --input 5200 --output 830 --cache-read 4000 --model claude-opus-4-8",
        ],
        gotchas: &[
            "Call after any significant LLM turn; without it Lens shows only the CTXone-side view, not real consumption.",
        ],
        related: &["stats", "tokens"],
    },
    HelpDoc {
        feature: "worktree",
        group: "workflow",
        synopsis: "Plan-scoped git worktrees: isolated files + HEAD per unit of work so parallel agents don't clobber each other.",
        syntax: "ctx worktree start|list|finish <plan> [--from <ref>] [--shared-target] [--push] [--keep]",
        params: &[
            p!(
                "plan",
                true,
                "Plan name; the worktree is ../<repo>-wt-<plan> on branch plan/<plan>."
            ),
            p!(
                "--shared-target",
                false,
                "start: share one Rust build cache (<repo>/.wt-target) instead of a per-worktree target/."
            ),
            p!(
                "--push",
                false,
                "finish: git push the target branch after merging."
            ),
        ],
        examples: &[
            "ctx worktree start add-oauth",
            "ctx worktree list",
            "ctx worktree finish add-oauth --push",
        ],
        gotchas: &[
            "The CLI can't cd you — `start` prints the path; open your session there.",
            "`finish` refuses on a dirty worktree and never switches the main checkout's HEAD; it force-tears-down (removes target/ too).",
        ],
        related: &["plan_new", "plan_start", "plan_done"],
    },
];

/// Rough token estimate (4 chars/token) — mirrors recall's fast estimator so
/// the disclosure cost is legible and comparable to the savings numbers.
fn est_tokens(doc: &HelpDoc) -> usize {
    let mut chars = doc.synopsis.len() + doc.syntax.len();
    for p in doc.params {
        chars += p.name.len() + p.desc.len();
    }
    for e in doc.examples {
        chars += e.len();
    }
    for g in doc.gotchas {
        chars += g.len();
    }
    for r in doc.related {
        chars += r.len();
    }
    chars / 4
}

fn doc_json(doc: &HelpDoc) -> Value {
    json!({
        "feature": doc.feature,
        "owner": OWNER,
        "version": version(),
        "synopsis": doc.synopsis,
        "syntax": doc.syntax,
        "params": doc.params,
        "examples": doc.examples,
        "gotchas": doc.gotchas,
        "related": doc.related,
        "help_tokens": est_tokens(doc),
    })
}

/// A score at or above this means we matched the feature NAME (exact or
/// substring) — confident enough to return that one doc. Below it, we only had
/// weak synopsis-word overlap, so we disambiguate instead of guessing.
const STRONG_MATCH: u32 = 200;

/// Score a doc against a lowercased query. Higher is better; 0 = no match.
/// Feature-name matches dominate; synopsis-word overlap only breaks ties.
/// `group` is deliberately NOT part of the match text — it's identical across a
/// whole group and would make every sibling tie on the group word.
fn score(doc: &HelpDoc, q: &str) -> u32 {
    let feat = doc.feature.to_lowercase();
    if feat == q {
        return 1000;
    }
    let mut s = 0u32;
    if feat.contains(q) || q.contains(&feat) {
        s += STRONG_MATCH;
    }
    let syn = doc.synopsis.to_lowercase();
    for tok in q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
    {
        if feat.contains(tok) {
            s += 100;
        }
        if syn.contains(tok) {
            s += 20;
        }
    }
    s
}

/// Grouped catalog for the no-arg overview — every feature, so agents discover
/// the full surface rather than only what they already know.
pub fn catalog() -> Value {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for doc in REGISTRY {
        groups.entry(doc.group).or_default().push(json!({
            "feature": doc.feature,
            "synopsis": doc.synopsis,
        }));
    }
    json!({
        "owner": OWNER,
        "version": version(),
        "usage": "Call `help <feature>` for exact syntax, examples, and gotchas before using a feature.",
        "groups": groups,
        "feature_count": REGISTRY.len(),
    })
}

/// The response for the `help` tool / endpoint. `topic` None or empty -> catalog.
pub fn respond(topic: Option<&str>) -> Value {
    let q = topic.map(|t| t.trim().to_lowercase()).unwrap_or_default();
    if q.is_empty() {
        return catalog();
    }
    let mut ranked: Vec<(&HelpDoc, u32)> = REGISTRY
        .iter()
        .map(|d| (d, score(d, &q)))
        .filter(|(_, s)| *s > 0)
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.feature.cmp(b.0.feature)));

    match ranked.first() {
        // Confident: matched the feature name — return that one doc.
        Some((doc, s)) if *s >= STRONG_MATCH => {
            let mut out = doc_json(doc);
            let also: Vec<&str> = ranked
                .iter()
                .skip(1)
                .take(4)
                .map(|(d, _)| d.feature)
                .collect();
            if let Some(obj) = out.as_object_mut() {
                obj.insert("also".into(), json!(also));
            }
            out
        }
        // Weak: only synopsis-word overlap — disambiguate instead of guessing
        // wrong. Return the candidates so the agent (or user) picks.
        Some(_) => json!({
            "query": q,
            "owner": OWNER,
            "matches": ranked.iter().take(6).map(|(d, _)| json!({
                "feature": d.feature,
                "synopsis": d.synopsis,
            })).collect::<Vec<_>>(),
            "hint": "No exact feature match — `help <feature>` for one of these, or `help` for the full catalog.",
        }),
        None => json!({
            "not_found": q,
            "owner": OWNER,
            "did_you_mean": REGISTRY.iter().map(|d| d.feature).collect::<Vec<_>>(),
            "hint": "No ctx feature matched. It may be an `asd` feature — try `asd help <topic>`.",
        }),
    }
}

/// Local resolve with a cross-binary proxy fallback: if `topic` doesn't match
/// any local feature, consult the shared index for other tools and ask the
/// owning binary directly (e.g. ctx proxies an unknown topic to `asd help`).
///
/// `allow_proxy` is false for the proxied child call (via `--no-proxy` / the
/// `no_proxy` query param), collapsing this to a pure local `respond` — the
/// single-hop loop guard. A successful proxy annotates `proxied_from`.
pub fn resolve(topic: Option<&str>, allow_proxy: bool) -> Value {
    let local = respond(topic);
    if !allow_proxy || local.get("not_found").is_none() {
        return local;
    }
    let Some(t) = topic.map(str::trim).filter(|t| !t.is_empty()) else {
        return local;
    };
    let Some(index) = read_index() else {
        return local;
    };
    let Some(tools) = index.get("tools").and_then(|v| v.as_object()) else {
        return local;
    };
    for tool in tools.keys().filter(|k| k.as_str() != OWNER) {
        let Some((bin, args)) = json_invocation(tool, t) else {
            continue;
        };
        let Ok(out) = std::process::Command::new(bin).args(&args).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        if let Ok(mut v) = serde_json::from_slice::<Value>(&out.stdout)
            && v.get("feature").is_some()
            && let Some(obj) = v.as_object_mut()
        {
            obj.insert("proxied_from".into(), json!(tool));
            return v;
        }
    }
    local
}

/// Read the shared cross-tool help index, if present.
fn read_index() -> Option<Value> {
    let path = if let Some(p) = std::env::var_os("AGENTSTATE_HELP_INDEX") {
        std::path::PathBuf::from(p)
    } else {
        std::path::PathBuf::from(std::env::var_os("HOME")?)
            .join(".config/agentstate/help-index.json")
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// How to invoke a given tool's `help` for JSON output, with proxy disabled so
/// the child never bounces back (single-hop guard).
fn json_invocation(tool: &str, topic: &str) -> Option<(&'static str, Vec<String>)> {
    match tool {
        "ctx" => Some((
            "ctx",
            vec![
                "help".into(),
                topic.into(),
                "--format".into(),
                "json".into(),
                "--no-proxy".into(),
            ],
        )),
        "asd" => Some((
            "asd",
            vec![
                "help".into(),
                topic.into(),
                "--agent".into(),
                "--no-proxy".into(),
            ],
        )),
        _ => None,
    }
}

/// The lightweight manifest this binary publishes to the shared help index so a
/// unified `help` can route across binaries without compile-time coupling.
pub fn manifest() -> Value {
    json!({
        "tool": OWNER,
        "version": version(),
        "features": REGISTRY.iter().map(|d| json!({
            "feature": d.feature,
            "synopsis": d.synopsis,
            "group": d.group,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_feature_name_wins() {
        let v = respond(Some("remember"));
        assert_eq!(v["feature"], "remember");
        assert_eq!(v["owner"], "ctx");
        assert!(v["help_tokens"].as_u64().unwrap() > 0);
    }

    #[test]
    fn strong_name_match_returns_single_doc() {
        // A feature-name word ("plan") is a strong match -> one doc.
        let v = respond(Some("plan start"));
        assert_eq!(v["feature"], "plan_start");
    }

    #[test]
    fn weak_phrase_disambiguates_not_guesses() {
        // "save a memory" has no feature-name hit — must NOT confidently pick
        // one doc; it returns candidates for the caller to choose.
        let v = respond(Some("save a memory"));
        assert!(
            v.get("feature").is_none(),
            "should not guess a single doc: {v}"
        );
        assert!(v["matches"].is_array(), "expected disambiguation list: {v}");
    }

    #[test]
    fn empty_topic_returns_catalog() {
        let v = respond(None);
        assert_eq!(
            v["feature_count"].as_u64().unwrap() as usize,
            REGISTRY.len()
        );
        assert!(v["groups"].is_object());
    }

    #[test]
    fn unknown_topic_suggests() {
        let v = respond(Some("zzzzznope"));
        assert_eq!(v["not_found"], "zzzzznope");
        assert!(v["did_you_mean"].is_array());
    }

    #[test]
    fn manifest_lists_every_feature() {
        let m = manifest();
        assert_eq!(m["tool"], "ctx");
        assert_eq!(m["features"].as_array().unwrap().len(), REGISTRY.len());
    }
}
