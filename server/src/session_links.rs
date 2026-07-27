//! Deriving a session's plan/task associations from its transcript.
//!
//! The Lens has long guessed at this client-side: scan each turn's tool calls
//! for `git commit -m "…"`, pull a `(plan t-NNN)` trailer out of the message,
//! and show it. That derivation is recomputed on every render, persists
//! nothing, validates nothing, and only reaches whatever turns the browser
//! currently holds. This moves it server-side, once, into a
//! `/session_links/<sid>` sidecar — mirroring the `/plan_links` precedent.
//!
//! What this can and cannot know is worth stating plainly: the transcript
//! records the commit *command*, not its result, so there is no SHA and no way
//! to tell a commit that landed from one that failed. The join key is the
//! task ref a human wrote in the message. So a link is *evidence a session
//! worked on a task*, not proof a commit exists — which is exactly why each
//! link is validated against the plan/task actually existing and carries that
//! verdict rather than being silently dropped.

use serde::Serialize;

/// One derived association between a session and a plan task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionLink {
    pub plan: String,
    pub task: String,
    /// The commit subject the ref was found in — the evidence, kept so a
    /// reader can see *why* the link was drawn.
    pub evidence: String,
    /// Turn index the evidence came from, for jumping to it in the transcript.
    pub turn_index: usize,
    /// Whether `plan`/`task` actually exist in this namespace. A `false` is
    /// still recorded: a reference to a since-deleted plan is evidence the
    /// work happened, and dropping it would hide that.
    pub validated: bool,
}

/// A commit command as it appears in a stored tool call.
///
/// Only the fields the derivation reads. The turn index is threaded through so
/// a link can point back at its evidence.
pub struct Candidate<'a> {
    pub turn_index: usize,
    pub command: &'a str,
}

/// Task refs mentioned in a commit message: `(plan-name t-NNN)`.
///
/// The trailer convention this repo uses. Case-insensitive on the type, and
/// the plan name is `[a-z][a-z0-9-]*` so a bare `(see t-4)` without a real
/// plan name still parses — validation, not the regex, decides if it is real.
fn task_refs(message: &str) -> Vec<(String, String)> {
    // Hand-rolled rather than a regex dep: find `(word tNNN)` groups.
    let mut out = Vec::new();
    let bytes = message.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'(' {
            i += 1;
            continue;
        }
        let Some(close) = message[i + 1..].find(')') else {
            break;
        };
        let inner = &message[i + 1..i + 1 + close];
        if let Some((plan, task)) = parse_ref(inner) {
            out.push((plan, task));
        }
        i += 1 + close + 1;
    }
    out
}

/// `plan-name t-004` → `(plan, normalized-task)`, or None.
///
/// Task ids are zero-padded to `t-{:03}` so `(plan t-4)` matches a minted
/// `t-004` — the client regex's `t-\d+` accepted `t-4` and then never
/// resolved it.
fn parse_ref(inner: &str) -> Option<(String, String)> {
    let mut parts = inner.split_whitespace();
    let plan = parts.next()?;
    let task = parts.next()?;
    if parts.next().is_some() {
        return None; // more than two tokens: not a clean ref
    }
    // plan: starts alpha, then [a-z0-9-]
    let mut chars = plan.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !plan.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let num = task
        .strip_prefix("t-")
        .or_else(|| task.strip_prefix("T-"))?;
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = num.parse().ok()?;
    Some((plan.to_ascii_lowercase(), format!("t-{n:03}")))
}

/// Every `-m "…"` / `-m '…'` message body in a `git commit` command.
///
/// Handles both quote styles and skips `--dry-run` — improvements over the
/// client regex, which was double-quote-only and matched dry runs.
fn commit_messages(command: &str) -> Vec<String> {
    // A command that only mentions the phrase (an echo, a --dry-run) is not a
    // commit. Require `git commit` and reject an explicit dry run.
    if !command.contains("git commit") || command.contains("--dry-run") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = command.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        // Look for `-m` followed by whitespace then a quote.
        if &bytes[i..i + 2] == b"-m" && bytes[i + 2].is_ascii_whitespace() {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                let quote = bytes[j];
                if let Some(end_rel) = find_close_quote(&command[j + 1..], quote) {
                    let body = &command[j + 1..j + 1 + end_rel];
                    out.push(unescape(body));
                    i = j + 1 + end_rel + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Index of the closing quote, honouring backslash escapes for `"`.
fn find_close_quote(s: &str, quote: u8) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if quote == b'"' && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == quote {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("\\n", "\n").replace("\\\"", "\"")
}

/// Derive the links for a set of commit candidates.
///
/// `exists` decides validation — the caller wires it to the plan/task store,
/// so this stays pure and testable. Deduped on (plan, task): a task worked on
/// across many commits is one link, attributed to the first commit that named
/// it.
pub fn derive(candidates: &[Candidate], exists: impl Fn(&str, &str) -> bool) -> Vec<SessionLink> {
    let mut out: Vec<SessionLink> = Vec::new();
    for c in candidates {
        for msg in commit_messages(c.command) {
            let subject = msg.lines().next().unwrap_or("").trim().to_string();
            for (plan, task) in task_refs(&msg) {
                if out.iter().any(|l| l.plan == plan && l.task == task) {
                    continue;
                }
                let validated = exists(&plan, &task);
                out.push(SessionLink {
                    plan,
                    task,
                    evidence: subject.clone(),
                    turn_index: c.turn_index,
                    validated,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(i: usize, cmd: &str) -> Candidate {
        Candidate {
            turn_index: i,
            command: cmd,
        }
    }

    #[test]
    fn extracts_and_normalizes_a_task_ref() {
        let links = derive(
            &[cand(3, r#"git commit -m "feat: lens board (ctxone t-4)""#)],
            |_, _| true,
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].plan, "ctxone");
        // t-4 normalized to the minted t-004.
        assert_eq!(links[0].task, "t-004");
        assert_eq!(links[0].turn_index, 3);
        assert!(links[0].validated);
    }

    #[test]
    fn handles_single_quotes_and_skips_dry_runs() {
        // Single-quoted body: the client regex missed these.
        let a = derive(
            &[cand(0, "git commit -m 'fix: thing (plan t-012)'")],
            |_, _| true,
        );
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].task, "t-012");

        // A dry run is not a commit.
        let b = derive(
            &[cand(0, r#"git commit --dry-run -m "nope (plan t-001)""#)],
            |_, _| true,
        );
        assert!(b.is_empty());

        // Merely mentioning the phrase is not a commit.
        let c = derive(
            &[cand(0, r#"echo "run git commit -m foo (plan t-001)""#)],
            |_, _| true,
        );
        assert!(c.is_empty());
    }

    #[test]
    fn records_unvalidated_refs_rather_than_dropping_them() {
        // A reference to a plan that no longer exists is still evidence.
        let links = derive(
            &[cand(1, r#"git commit -m "chore (ghost-plan t-999)""#)],
            |plan, _| plan != "ghost-plan",
        );
        assert_eq!(links.len(), 1);
        assert!(!links[0].validated);
    }

    #[test]
    fn dedupes_a_task_worked_on_across_commits() {
        let links = derive(
            &[
                cand(1, r#"git commit -m "feat: part 1 (p t-001)""#),
                cand(4, r#"git commit -m "feat: part 2 (p t-001)""#),
            ],
            |_, _| true,
        );
        assert_eq!(links.len(), 1);
        // Attributed to the FIRST commit that named it.
        assert_eq!(links[0].turn_index, 1);
    }

    #[test]
    fn multiple_refs_and_multiple_m_flags() {
        let links = derive(
            &[cand(
                0,
                r#"git commit -m "feat: two things (plan-a t-001)" -m "body (plan-b t-2)""#,
            )],
            |_, _| true,
        );
        assert_eq!(links.len(), 2);
        assert_eq!(links[1].task, "t-002");
    }

    #[test]
    fn ignores_garbage_in_parens() {
        // Not a ref: too many tokens, or no task.
        let links = derive(
            &[cand(
                0,
                r#"git commit -m "wip (see the other thing) and (t-1 extra bad)""#,
            )],
            |_, _| true,
        );
        assert!(links.is_empty());
    }
}
