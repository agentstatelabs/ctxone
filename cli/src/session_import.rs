//! Session discovery and onboarding for `ctx session list|import|ignore`.
//!
//! The low-level engine is `ctx ingest-session` (whole-machine or targeted
//! sweeps). These commands sit on top of it and are the surface a *person*
//! uses: discover what transcripts exist on this machine, show them as a
//! stable, paged, numbered list, and import a chosen subset — instead of
//! auto-importing everything the moment the hub starts.
//!
//! ## The stable numbering contract
//!
//! `list` and `import` share one ordering so that "number 7 in the list" is
//! "number 7 to import". Discovery is deterministic — every source enumerates
//! its transcripts, we sort the union by last-activity (newest first) with the
//! namespaced id as a tiebreak — so two calls a moment apart agree on numbers
//! without persisting anything. A transcript created between the two calls
//! shifts later numbers, which is why `import` echoes what each number resolved
//! to before doing any work.
//!
//! ## Privacy skip-list
//!
//! `ignore` records a session id in `~/.ctxone/ignored-sessions.txt`. The list
//! is consulted by the import engine itself (see `load_ignored`), so an ignored
//! session is skipped by BOTH a manual `import` and the hub's background sweep —
//! the transcript stays on disk and simply never enters the graph.

use crate::sources::all_sources;
use std::collections::HashSet;
use std::io::Write as _;
use std::path::PathBuf;

/// Sessions shown per page in `list` (the user asked for pages of 25).
pub const PAGE_SIZE: usize = 25;

/// One discovered transcript, in the stable numbered ordering shared by
/// `list` and `import`.
pub struct Discovered {
    /// 1-based position in the stable ordering. `import 7` selects the entry
    /// whose `number` is 7.
    pub number: usize,
    pub source_id: &'static str,
    /// Project grouping (e.g. `Project/CTXone`), for display only.
    pub project: String,
    /// The stored, source-namespaced id — what `import`/`ignore` key on.
    pub id: String,
    /// Epoch-seconds of last activity, when known (transcript mtime).
    pub last_activity: Option<i64>,
}

/// Discover every transcript on this machine, optionally restricted to one
/// `source` id, returned in the stable numbered ordering.
pub fn discover(source_filter: Option<&str>) -> Vec<Discovered> {
    let mut rows: Vec<Discovered> = Vec::new();
    for src in all_sources() {
        if let Some(want) = source_filter {
            if src.id() != want {
                continue;
            }
        }
        if !src.is_available() {
            continue;
        }
        for r in src.discover_all() {
            rows.push(Discovered {
                number: 0, // assigned after the global sort
                source_id: src.id(),
                last_activity: src.last_activity(&r),
                project: r.label.clone(),
                id: r.namespaced_id(src.id()),
            });
        }
    }
    // Newest first; namespaced id breaks ties so the order is total and stable
    // across calls (mtimes collide for sessions written in the same second).
    rows.sort_by(|a, b| {
        b.last_activity
            .cmp(&a.last_activity)
            .then_with(|| a.id.cmp(&b.id))
    });
    for (i, row) in rows.iter_mut().enumerate() {
        row.number = i + 1;
    }
    rows
}

/// Path to the privacy skip-list. Sits beside `config.toml` so wiping the graph
/// db never loses the user's "keep these private" choices.
pub fn ignore_path() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        dirs::config_dir().map(|d| d.join("ctxone"))
    } else {
        dirs::home_dir().map(|h| h.join(".ctxone"))
    };
    base.unwrap_or_else(|| PathBuf::from("."))
        .join("ignored-sessions.txt")
}

/// The set of session ids the user has marked private. Read by the import
/// engine as well as `list`, so one file governs both surfaces.
///
/// Format is one namespaced id per line; blank lines and `#` comments are
/// ignored so the file stays hand-editable.
pub fn load_ignored() -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(ignore_path()) else {
        return HashSet::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Persist a skip-list, sorted for a stable, diffable file.
fn save_ignored(ids: &HashSet<String>) -> std::io::Result<()> {
    let path = ignore_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut sorted: Vec<&String> = ids.iter().collect();
    sorted.sort();
    let mut body = String::from(
        "# Sessions ctx will never import (privacy skip-list).\n\
         # One namespaced session id per line. Edit freely or use\n\
         # `ctx session ignore|unignore`.\n",
    );
    for id in sorted {
        body.push_str(id);
        body.push('\n');
    }
    std::fs::write(&path, body)
}

/// Add ids to the skip-list. Returns how many were newly added.
pub fn add_ignored(new_ids: &[String]) -> std::io::Result<usize> {
    let mut set = load_ignored();
    let before = set.len();
    for id in new_ids {
        set.insert(id.clone());
    }
    let added = set.len() - before;
    save_ignored(&set)?;
    Ok(added)
}

/// Remove ids from the skip-list. Returns how many were actually removed.
pub fn remove_ignored(ids: &[String]) -> std::io::Result<usize> {
    let mut set = load_ignored();
    let mut removed = 0;
    for id in ids {
        if set.remove(id) {
            removed += 1;
        }
    }
    save_ignored(&set)?;
    Ok(removed)
}

/// Format an epoch-seconds timestamp as `YYYY-MM-DD`, or `—` when unknown.
fn fmt_date(secs: Option<i64>) -> String {
    match secs.and_then(|s| chrono::DateTime::from_timestamp(s, 0)) {
        Some(dt) => dt.format("%Y-%m-%d").to_string(),
        None => "—".to_string(),
    }
}

/// Short, display-friendly form of a (possibly source-prefixed) session id.
fn short_id(id: &str) -> String {
    // Keep any `source:` prefix, shorten only the opaque tail.
    let (prefix, tail) = match id.split_once(':') {
        Some((p, t)) => (format!("{p}:"), t),
        None => (String::new(), id),
    };
    let tail = if tail.len() > 12 { &tail[..12] } else { tail };
    format!("{prefix}{tail}")
}

/// Per-row status shown in `list`.
#[derive(Clone, Copy)]
pub enum Status {
    New,
    Imported,
    Ignored,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::New => "new",
            Status::Imported => "imported",
            Status::Ignored => "ignored",
        }
    }
}

/// Classify a row against the already-imported set and the local skip-list.
/// Ignored wins over imported: the user's privacy choice is what they want to
/// see, even if an older import already happened.
pub fn status_for(id: &str, imported: &HashSet<String>, ignored: &HashSet<String>) -> Status {
    if ignored.contains(id) {
        Status::Ignored
    } else if imported.contains(id) {
        Status::Imported
    } else {
        Status::New
    }
}

/// Render one page of the numbered list to stdout.
pub fn print_page(
    rows: &[Discovered],
    page: usize,
    imported: &HashSet<String>,
    ignored: &HashSet<String>,
) {
    let total = rows.len();
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let page = page.clamp(1, pages);
    let start = (page - 1) * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(total);

    println!(
        "{:>4}  {:<10}  {:<14}  {:<10}  {:<9}  {}",
        "#", "DATE", "ID", "SOURCE", "STATUS", "PROJECT"
    );
    for row in &rows[start..end] {
        let st = status_for(&row.id, imported, ignored);
        println!(
            "{:>4}  {:<10}  {:<14}  {:<10}  {:<9}  {}",
            row.number,
            fmt_date(row.last_activity),
            short_id(&row.id),
            row.source_id,
            st.as_str(),
            row.project,
        );
    }
    println!(
        "\nPage {}/{} · {} session{} total  ·  showing {}–{}",
        page,
        pages,
        total,
        if total == 1 { "" } else { "s" },
        if total == 0 { 0 } else { start + 1 },
        end
    );
    if pages > 1 {
        println!("Next page:  ctx session list --page {}", (page % pages) + 1);
    }
    println!(
        "Import:     ctx session import <numbers|ranges|all>   (e.g. `import 1-10 15`, `import all`)"
    );
}

/// Parse selectors like `1`, `3-9`, `all`, or a literal id into the set of
/// namespaced ids they resolve to, against the stable `rows` ordering.
///
/// Returns `(ids, unmatched_tokens)` — a token that resolved to nothing (an
/// out-of-range number, an id we didn't discover) is reported rather than
/// silently dropped, so a typo doesn't look like a clean no-op.
pub fn resolve_selectors(rows: &[Discovered], tokens: &[String]) -> (HashSet<String>, Vec<String>) {
    let mut ids = HashSet::new();
    let mut unmatched = Vec::new();
    let by_number: std::collections::HashMap<usize, &Discovered> =
        rows.iter().map(|r| (r.number, r)).collect();
    let known_ids: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();

    for tok in tokens {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        if tok.eq_ignore_ascii_case("all") {
            ids.extend(rows.iter().map(|r| r.id.clone()));
            continue;
        }
        if let Some((a, b)) = tok.split_once('-') {
            // A numeric range like `3-9`.
            if let (Ok(lo), Ok(hi)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
                let mut any = false;
                for n in lo..=hi {
                    if let Some(row) = by_number.get(&n) {
                        ids.insert(row.id.clone());
                        any = true;
                    }
                }
                if !any {
                    unmatched.push(tok.to_string());
                }
                continue;
            }
        }
        if let Ok(n) = tok.parse::<usize>() {
            match by_number.get(&n) {
                Some(row) => {
                    ids.insert(row.id.clone());
                }
                None => unmatched.push(tok.to_string()),
            }
            continue;
        }
        // Fall back to treating the token as a literal session id.
        if known_ids.contains(tok) {
            ids.insert(tok.to_string());
        } else {
            unmatched.push(tok.to_string());
        }
    }
    (ids, unmatched)
}

/// A one-line notice about extraction richness, printed before an import so the
/// user understands what a missing API key costs. Reads the same config the
/// engine does — it never prompts for or stores a secret.
pub fn extraction_notice() -> String {
    if crate::ingest::resolve_extraction_config().is_some() {
        "An extraction key is configured — importing in RICH mode \
         (topics and memories will be extracted for fuller session views)."
            .to_string()
    } else {
        "No extraction key configured — importing in PLAIN mode \
         (turns and token counts only; no topics/memories). Set a provider key \
         (env var or ~/.ctxone/keys/<provider>) and re-import for richer views."
            .to_string()
    }
}

/// Interactive selection loop: page through the list and collect selectors typed
/// at a prompt. Returns the resolved id set, or `None` if the user quit.
///
/// Commands: `n`/`p` page, a page number, `1-10 15` to add selectors, `all`,
/// `done` to import the accumulated selection, `q` to abort.
pub fn interactive_select(
    rows: &[Discovered],
    imported: &HashSet<String>,
    ignored: &HashSet<String>,
) -> Option<HashSet<String>> {
    let pages = rows.len().div_ceil(PAGE_SIZE).max(1);
    let mut page = 1usize;
    let mut chosen: HashSet<String> = HashSet::new();
    let stdin = std::io::stdin();

    loop {
        print_page(rows, page, imported, ignored);
        println!(
            "\nSelected so far: {}.  Commands: [n]ext [p]rev <page#> \
             <sel e.g. 1-10 15> all done q",
            chosen.len()
        );
        print!("select> ");
        let _ = std::io::stdout().flush();

        let mut line = String::new();
        if stdin.read_line(&mut line).ok()? == 0 {
            // EOF (piped/non-interactive) — treat as "done" so scripts still work.
            break;
        }
        let line = line.trim();
        match line {
            "" => continue,
            "q" | "quit" | "exit" => return None,
            "done" | "import" => break,
            "n" | "next" => {
                page = (page % pages) + 1;
                continue;
            }
            "p" | "prev" => {
                page = if page <= 1 { pages } else { page - 1 };
                continue;
            }
            _ => {}
        }
        // A bare page number jumps; anything else is treated as selectors.
        if let Ok(n) = line.parse::<usize>() {
            if n >= 1 && n <= pages {
                page = n;
                continue;
            }
        }
        let tokens: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let (ids, unmatched) = resolve_selectors(rows, &tokens);
        if !unmatched.is_empty() {
            println!("  ignored unrecognized: {}", unmatched.join(", "));
        }
        let added = ids.difference(&chosen).count();
        chosen.extend(ids);
        println!("  +{added} selected");
    }
    Some(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(number: usize, id: &str) -> Discovered {
        Discovered {
            number,
            source_id: "claude",
            project: "P".into(),
            id: id.into(),
            last_activity: Some(number as i64),
        }
    }

    fn rows() -> Vec<Discovered> {
        vec![row(1, "aaa"), row(2, "bbb"), row(3, "ccc"), row(4, "ddd")]
    }

    #[test]
    fn selects_single_numbers_and_ranges() {
        let (ids, un) = resolve_selectors(&rows(), &["1".into(), "3-4".into()]);
        assert!(un.is_empty());
        assert_eq!(
            ids,
            HashSet::from(["aaa".into(), "ccc".into(), "ddd".into()])
        );
    }

    #[test]
    fn all_selects_everything() {
        let (ids, _) = resolve_selectors(&rows(), &["all".into()]);
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn reversed_range_is_normalized() {
        let (ids, un) = resolve_selectors(&rows(), &["4-2".into()]);
        assert!(un.is_empty());
        assert_eq!(ids.len(), 3); // 2,3,4
    }

    #[test]
    fn literal_id_resolves_and_unknown_is_reported() {
        let (ids, un) = resolve_selectors(&rows(), &["bbb".into(), "99".into(), "zzz".into()]);
        assert_eq!(ids, HashSet::from(["bbb".into()]));
        assert_eq!(un, vec!["99".to_string(), "zzz".to_string()]);
    }

    #[test]
    fn status_prefers_ignored_over_imported() {
        let imported = HashSet::from(["x".to_string()]);
        let ignored = HashSet::from(["x".to_string()]);
        assert!(matches!(
            status_for("x", &imported, &ignored),
            Status::Ignored
        ));
        assert!(matches!(
            status_for("x", &imported, &HashSet::new()),
            Status::Imported
        ));
        assert!(matches!(status_for("y", &imported, &ignored), Status::New));
    }

    #[test]
    fn short_id_keeps_source_prefix() {
        assert_eq!(short_id("claude"), "claude");
        assert_eq!(short_id("codex:0123456789abcdefghij"), "codex:0123456789ab");
    }
}
