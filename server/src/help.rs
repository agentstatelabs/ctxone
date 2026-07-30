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
        syntax: "ctx plan-new <name> [--description <text>]",
        params: &[p!("name", true, "kebab-case plan name, used in paths.")],
        examples: &["ctx plan-new website-v2"],
        gotchas: &["Fails if a plan with that name already exists on the branch."],
        related: &["plan_add", "plan_start", "plan_next"],
    },
    HelpDoc {
        feature: "plan_add",
        group: "plans",
        synopsis: "Add a task to a plan; optionally assign it to a specific agent.",
        syntax: "ctx plan-add <plan> <title> [--priority ...] [--assigned-to <agent>] [--blocked-by <ids>]",
        params: &[
            p!("plan_id", true, "Plan to add the task to."),
            p!("title", true, "Imperative one-line task title."),
            p!("--priority", false, "critical|high|medium|low."),
        ],
        examples: &["ctx plan-add website-v2 \"Wire the nav\" --priority high"],
        gotchas: &[
            "Blockers must already exist in the plan; a plan may auto-lock past a done ratio (use --force).",
        ],
        related: &["plan_new", "plan_start", "plan_complete"],
    },
    HelpDoc {
        feature: "plan_start",
        group: "plans",
        synopsis: "Move a task from pending to in_progress; refuses if blockers aren't done.",
        syntax: "ctx plan-start <plan> <task_id>",
        params: &[
            p!("plan_id", true, "Plan holding the task."),
            p!("task_id", true, "Task to start (e.g. t-003)."),
        ],
        examples: &["ctx plan-start ctx-context-paradigm t-005"],
        gotchas: &[
            "Warns (doesn't block) if other tasks are already in_progress — finish or abandon stale ones.",
        ],
        related: &["plan_add", "plan_complete", "plan_next"],
    },
    HelpDoc {
        feature: "plan_complete",
        group: "plans",
        synopsis: "Mark a task done WITH PROOF (commit SHA strongest, then file path, then test name).",
        syntax: "ctx plan-complete <plan> <task_id> --proof <proof>",
        params: &[
            p!("plan_id", true, "Plan holding the task."),
            p!("task_id", true, "Task to complete."),
            p!(
                "--proof",
                true,
                "Evidence of completion; prefer a commit SHA."
            ),
        ],
        examples: &["ctx plan-complete ctx-context-paradigm t-005 --proof <sha>"],
        gotchas: &["Never use text-only proof when a SHA, file, or test name is available."],
        related: &["plan_start", "plan_abandon"],
    },
    HelpDoc {
        feature: "plan_next",
        group: "plans",
        synopsis: "Get the highest-priority pending task whose blockers are done; supports assigned_to='me'.",
        syntax: "ctx plan-next [--plan <name>] [--assigned-to me]",
        params: &[p!(
            "--assigned-to",
            false,
            "'me' maps to the caller's agent id for multi-agent orchestration."
        )],
        examples: &["ctx plan-next --plan ctx-context-paradigm"],
        gotchas: &[
            "The plan IS the orchestration layer — agents coordinate through shared task state.",
        ],
        related: &["plan_start", "plan_list"],
    },
    HelpDoc {
        feature: "plan_list",
        group: "plans",
        synopsis: "List active plans and their task counts — call at session start to see what's in flight.",
        syntax: "ctx plan-list",
        params: &[],
        examples: &["ctx plan-list"],
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
