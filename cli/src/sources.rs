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
        Self::jsonl_files(&dir)
            .into_iter()
            .map(|path| SessionRef {
                label: label.clone(),
                path,
                native_id: None,
            })
            .collect()
    }

    fn parse(&self, session: &SessionRef) -> Vec<Turn> {
        crate::ingest::parse_turns(&session.path)
    }
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Every known source, in the order they should be scanned.
pub fn all_sources() -> Vec<Box<dyn SessionSource>> {
    vec![Box::new(ClaudeCode)]
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
            path: PathBuf::from("/tmp/abc-123.jsonl"),
            native_id: None,
        };
        assert_eq!(r.session_id(), "abc-123");
    }

    #[test]
    fn session_id_prefers_native_id() {
        let r = SessionRef {
            label: "x/y".into(),
            path: PathBuf::from("/tmp/state.vscdb"),
            native_id: Some("composer-7".into()),
        };
        assert_eq!(r.session_id(), "composer-7");
    }

    #[test]
    fn group_by_label_keeps_projects_together() {
        let mk = |label: &str, p: &str| SessionRef {
            label: label.into(),
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
