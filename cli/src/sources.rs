//! Session sources: the seam between "where transcripts live" and "what we
//! do with them".
//!
//! Ingestion was originally Claude Code all the way down — discovery walked
//! `~/.claude/projects`, the parser assumed Claude's JSONL, and `metrics.rs`
//! carried a second copy of the same directory walk. Adding Codex, Cursor or
//! Gemini to that shape would have meant a fourth and fifth copy.
//!
//! A [`SessionSource`] owns the two things that genuinely differ per agent:
//! **where** its transcripts live and **how** to turn one into [`Turn`]s.
//! Everything downstream — memory extraction, token posting, title
//! derivation — works on `Turn` and does not care which agent produced it.
//!
//! Claude Code is the only implementation today; it is a straight move of the
//! previous behaviour, not a rewrite.
//!
//! ## Adding a source
//!
//! Implement the trait and add the source to [`all_sources`]. Two shapes are
//! already known not to fit perfectly:
//!
//! - **Cursor** keeps conversations in a SQLite db, not one file per session,
//!   so its [`SessionRef::path`] will point at the db and `native_id` will
//!   carry the composer id that selects a row. The trait is deliberately
//!   `parse(&self, &SessionRef)` rather than `parse(&Path)` so that works.
//! - **Gemini CLI** nests sub-agent sessions inside a per-session directory,
//!   so its `discover_all` will emit more refs than there are top-level files.

use crate::ingest::Turn;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One importable session, as located by a [`SessionSource`].
#[derive(Debug, Clone)]
pub struct SessionRef {
    /// Human-readable project grouping, e.g. `Project/CTXone`. Used for
    /// per-project reporting in `ctx ingest-session --all`.
    pub label: String,
    /// What to read. Usually the transcript file; for db-backed sources it is
    /// the database, with `native_id` selecting the conversation.
    pub path: PathBuf,
    /// The agent's own id for this session, when it is known before parsing.
    /// `None` means the caller derives one (Claude Code uses the file stem).
    pub native_id: Option<String>,
    /// Absolute working directory the session ran in, when the source records
    /// one. This is what routes a session to its workspace — it is resolved
    /// against the project registry, unlike [`label`](Self::label), which is
    /// derived from Claude Code's hashed directory name and cannot tell a
    /// literal dash from a path separator.
    ///
    /// `None` for sources that do not record it; such sessions fall back to
    /// the default namespace rather than being guessed at.
    pub cwd: Option<String>,
}

impl SessionRef {
    /// The session id to store, falling back to the file stem.
    ///
    /// Ids are only unique *within* a source — Codex uses a uuid, Cursor a
    /// composer id — so callers that mix sources must namespace this.
    pub fn session_id(&self) -> String {
        self.native_id.clone().unwrap_or_else(|| {
            self.path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        })
    }

    /// The stored session id, namespaced by source so two agents cannot
    /// collide on the same underlying id.
    ///
    /// Claude Code is deliberately **not** prefixed: its ids are already in
    /// the wild, keyed into every existing session row, memory tag and
    /// `/sessions/{id}/turns` path. Prefixing it would orphan all of that and
    /// re-import every session as a duplicate. New sources pay the prefix;
    /// the incumbent keeps its ids.
    pub fn namespaced_id(&self, source_id: &str) -> String {
        let id = self.session_id();
        if source_id == "claude" {
            id
        } else {
            format!("{}:{}", source_id, id)
        }
    }
}

/// An agent whose sessions we can import.
pub trait SessionSource {
    /// Stable machine id used by `--source` and stored on the session.
    fn id(&self) -> &'static str;

    /// Human label shown in Lens (the `source` field on a session).
    fn label(&self) -> &'static str;

    /// Whether this agent leaves anything on this machine. Callers use it to
    /// skip sources the user does not have installed rather than reporting
    /// them as empty.
    fn is_available(&self) -> bool;

    /// Every session this source can see, grouped by project label.
    fn discover_all(&self) -> Vec<SessionRef>;

    /// Sessions belonging to one project directory. Sources with no notion of
    /// a per-project transcript may return an empty vec.
    fn discover_for_project(&self, project_dir: &Path) -> Vec<SessionRef>;

    /// Read one session into source-neutral turns. Returns an empty vec on
    /// unreadable or unparseable input — a bad transcript must not abort a
    /// whole-machine scan.
    fn parse(&self, session: &SessionRef) -> Vec<Turn>;
}

// ── Claude Code ───────────────────────────────────────────────────────────────

/// Claude Code: one JSONL file per session under
/// `~/.claude/projects/<path-with-slashes-replaced-by-dashes>/`.
pub struct ClaudeCode;

impl ClaudeCode {
    fn projects_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".claude")
            .join("projects")
    }

    /// Recover a readable label from Claude Code's hashed directory name.
    ///
    /// The hash is `project_path.replace('/', "-")`, so an absolute path leads
    /// with '-'. We take the last two components, which is enough to tell
    /// projects apart without printing the whole path.
    fn label_for(hash: &str) -> String {
        let Some(stripped) = hash.strip_prefix('-') else {
            return hash.to_string();
        };
        let parts: Vec<&str> = stripped.split('-').collect();
        if parts.len() >= 2 {
            format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
        } else {
            hash.to_string()
        }
    }

    /// Read `cwd` from the head of a transcript without parsing all of it.
    ///
    /// Every Claude Code entry carries top-level `cwd` and `gitBranch`; the
    /// first one that has it is enough to route the session to a workspace.
    /// Whole-machine scans touch every transcript and these reach tens of MB,
    /// so this stops at the first hit — the same reasoning as Codex's
    /// [`read_meta`](Codex::read_meta).
    ///
    /// Early lines are occasionally summary/meta records without `cwd`, hence
    /// scanning a few rather than trusting line 1.
    fn read_cwd(path: &Path) -> Option<String> {
        use std::io::{BufRead, BufReader};
        let f = std::fs::File::open(path).ok()?;
        for line in BufReader::new(f).lines().take(20).map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(cwd) = v.get("cwd").and_then(|x| x.as_str()) {
                if !cwd.is_empty() {
                    return Some(cwd.to_string());
                }
            }
        }
        None
    }

    /// `.jsonl` files in `dir`, oldest first by mtime.
    fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
            .collect();
        files.sort_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
        files
    }
}

impl SessionSource for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn is_available(&self) -> bool {
        Self::projects_dir().is_dir()
    }

    fn discover_all(&self) -> Vec<SessionRef> {
        let Ok(entries) = std::fs::read_dir(Self::projects_dir()) else {
            return vec![];
        };

        // Collect per project first so projects stay sorted by label and files
        // stay oldest-first within a project — callers print per-project counts.
        let mut per_project: Vec<(String, Vec<PathBuf>)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter_map(|path| {
                let hash = path.file_name()?.to_string_lossy().to_string();
                let files = Self::jsonl_files(&path);
                (!files.is_empty()).then(|| (Self::label_for(&hash), files))
            })
            .collect();
        per_project.sort_by(|a, b| a.0.cmp(&b.0));

        per_project
            .into_iter()
            .flat_map(|(label, files)| {
                files.into_iter().map(move |path| SessionRef {
                    label: label.clone(),
                    cwd: Self::read_cwd(&path),
                    path,
                    native_id: None, // file stem is the session id
                })
            })
            .collect()
    }

    fn discover_for_project(&self, project_dir: &Path) -> Vec<SessionRef> {
        let hash = project_dir.to_string_lossy().replace('/', "-");
        let dir = Self::projects_dir().join(&hash);
        if !dir.exists() {
            return vec![];
        }
        let label = Self::label_for(&hash);
        // The caller already named the directory, so prefer it over re-reading
        // the transcript; fall back to the file for the odd session whose cwd
        // differs from the directory it was filed under.
        let dir_cwd = project_dir.to_string_lossy().to_string();
        Self::jsonl_files(&dir)
            .into_iter()
            .map(|path| SessionRef {
                label: label.clone(),
                cwd: Self::read_cwd(&path).or_else(|| Some(dir_cwd.clone())),
                path,
                native_id: None,
            })
            .collect()
    }

    fn parse(&self, session: &SessionRef) -> Vec<Turn> {
        crate::ingest::parse_turns(&session.path)
    }
}

// ── Codex ─────────────────────────────────────────────────────────────────────

/// Codex CLI / Desktop: one "rollout" JSONL per session under
/// `~/.codex/sessions/` (live) and `~/.codex/archived_sessions/`.
///
/// Every line is `{timestamp, type, payload}`. The types that matter:
///
/// - `session_meta` — one per file, first line: session id, `cwd`,
///   `originator`, `cli_version`, `model_provider`.
/// - `response_item` — the conversation. `payload.type` is `message`
///   (with `role`), `function_call`, `function_call_output`, `reasoning`, …
/// - `event_msg` with `payload.type == "token_count"` — usage. Carries both
///   `total_token_usage` (cumulative for the session) and `last_token_usage`
///   (this turn). We read the latter and sum, so a truncated file still
///   yields the usage of the turns it does contain.
pub struct Codex;

impl Codex {
    fn roots() -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let codex = home.join(".codex");
        vec![codex.join("sessions"), codex.join("archived_sessions")]
    }

    /// `rollout-*.jsonl` anywhere under `dir` — Codex nests live sessions in
    /// dated subdirectories, so this recurses rather than reading one level.
    fn rollout_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "jsonl").unwrap_or(false)
                    && p.file_name()
                        .map(|n| n.to_string_lossy().starts_with("rollout-"))
                        .unwrap_or(false)
                {
                    out.push(p);
                }
            }
        }
        out.sort_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
        out
    }

    /// Read the `session_meta` line without parsing the whole file.
    ///
    /// Discovery runs over every rollout on the machine, and these files reach
    /// tens of MB — reading only the first lines keeps a scan cheap. The meta
    /// record is the first line in practice; we scan a few in case that
    /// changes rather than assuming position.
    fn read_meta(path: &Path) -> Option<(String, String)> {
        use std::io::{BufRead, BufReader};
        let f = std::fs::File::open(path).ok()?;
        for line in BufReader::new(f).lines().take(10).map_while(Result::ok) {
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
                continue;
            }
            let p = v.get("payload")?;
            let id = p.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let cwd = p.get("cwd").and_then(|x| x.as_str()).unwrap_or("").to_string();
            return Some((id, cwd));
        }
        None
    }

    /// `/Users/user/Apps/Thing` -> `Apps/Thing`, matching how the Claude Code
    /// source labels projects so mixed listings read consistently.
    fn label_for_cwd(cwd: &str) -> String {
        let parts: Vec<&str> = cwd.trim_end_matches('/').split('/').filter(|s| !s.is_empty()).collect();
        match parts.len() {
            0 => "unknown".to_string(),
            1 => parts[0].to_string(),
            n => format!("{}/{}", parts[n - 2], parts[n - 1]),
        }
    }
}

impl SessionSource for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn is_available(&self) -> bool {
        Self::roots().iter().any(|r| r.is_dir())
    }

    fn discover_all(&self) -> Vec<SessionRef> {
        let mut refs: Vec<SessionRef> = Self::roots()
            .iter()
            .filter(|r| r.is_dir())
            .flat_map(|r| Self::rollout_files(r))
            .map(|path| {
                let (id, cwd) = Self::read_meta(&path).unwrap_or_default();
                SessionRef {
                    label: if cwd.is_empty() {
                        "unknown".to_string()
                    } else {
                        Self::label_for_cwd(&cwd)
                    },
                    cwd: (!cwd.is_empty()).then_some(cwd),
                    // Fall back to the filename's uuid when meta is missing;
                    // session_id() handles the None case via the file stem.
                    native_id: (!id.is_empty()).then_some(id),
                    path,
                }
            })
            .collect();
        // Group projects together and keep them label-sorted, matching Claude.
        refs.sort_by(|a, b| a.label.cmp(&b.label));
        refs
    }

    fn discover_for_project(&self, project_dir: &Path) -> Vec<SessionRef> {
        let want = project_dir.to_string_lossy().to_string();
        let want = want.trim_end_matches('/').to_string();
        self.discover_all()
            .into_iter()
            .filter(|r| {
                // Match on the recorded cwd, not the label, which is lossy.
                // `discover_all` already read it, so this no longer re-opens
                // every rollout on the machine to answer one project.
                r.cwd
                    .as_deref()
                    .map(|c| c.trim_end_matches('/') == want)
                    .unwrap_or(false)
            })
            .collect()
    }

    fn parse(&self, session: &SessionRef) -> Vec<Turn> {
        crate::codex::parse_rollout(&session.path)
    }
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Every known source, in the order they should be scanned.
pub fn all_sources() -> Vec<Box<dyn SessionSource>> {
    vec![Box::new(ClaudeCode), Box::new(Codex)]
}

/// Look up a source by its `--source` id.
pub fn source_by_id(id: &str) -> Option<Box<dyn SessionSource>> {
    all_sources().into_iter().find(|s| s.id() == id)
}

/// Group refs by project label, preserving discovery order within a group.
///
/// Discovery returns a flat list because that is the useful shape for
/// importing; the CLI wants `(label, files)` pairs for its per-project
/// reporting, which this restores without a second directory walk.
pub fn group_by_label(refs: Vec<SessionRef>) -> Vec<(String, Vec<PathBuf>)> {
    let mut out: Vec<(String, Vec<PathBuf>)> = vec![];
    for r in refs {
        match out.last_mut() {
            Some((label, files)) if *label == r.label => files.push(r.path),
            _ => out.push((r.label.clone(), vec![r.path])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_recovers_last_two_components() {
        assert_eq!(
            ClaudeCode::label_for("-Users-user-Documents-Project-CTXone"),
            "Project/CTXone"
        );
    }

    #[test]
    fn label_passes_through_unhashed_names() {
        assert_eq!(ClaudeCode::label_for("plain"), "plain");
    }

    #[test]
    fn session_id_falls_back_to_file_stem() {
        let r = SessionRef {
            label: "x/y".into(),
            cwd: None,
            path: PathBuf::from("/tmp/abc-123.jsonl"),
            native_id: None,
        };
        assert_eq!(r.session_id(), "abc-123");
    }

    #[test]
    fn session_id_prefers_native_id() {
        let r = SessionRef {
            label: "x/y".into(),
            cwd: None,
            path: PathBuf::from("/tmp/state.vscdb"),
            native_id: Some("composer-7".into()),
        };
        assert_eq!(r.session_id(), "composer-7");
    }

    #[test]
    fn claude_ids_are_not_namespaced_so_existing_rows_keep_working() {
        let r = SessionRef {
            label: "a/b".into(),
            cwd: None,
            path: PathBuf::from("/x/9117346d.jsonl"),
            native_id: None,
        };
        assert_eq!(r.namespaced_id("claude"), "9117346d");
    }

    #[test]
    fn other_sources_are_namespaced_to_avoid_collisions() {
        let r = SessionRef {
            label: "a/b".into(),
            cwd: None,
            path: PathBuf::from("/x/rollout-abc.jsonl"),
            native_id: Some("019e5540".into()),
        };
        assert_eq!(r.namespaced_id("codex"), "codex:019e5540");
    }

    #[test]
    fn codex_label_matches_the_claude_two_component_style() {
        assert_eq!(Codex::label_for_cwd("/Users/user/Apps/Thing"), "Apps/Thing");
        assert_eq!(Codex::label_for_cwd("/solo"), "solo");
        assert_eq!(Codex::label_for_cwd(""), "unknown");
    }

    #[test]
    fn group_by_label_keeps_projects_together() {
        let mk = |label: &str, p: &str| SessionRef {
            label: label.into(),
            cwd: None,
            path: PathBuf::from(p),
            native_id: None,
        };
        let grouped = group_by_label(vec![
            mk("a/b", "/1.jsonl"),
            mk("a/b", "/2.jsonl"),
            mk("c/d", "/3.jsonl"),
        ]);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].1.len(), 2);
        assert_eq!(grouped[1].0, "c/d");
    }
}
