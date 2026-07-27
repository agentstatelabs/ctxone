//! Project registry — maps codebases to CTXone namespaces.
//!
//! The registry lives in the hub's **own SQLite database** (the same file as
//! the ASG storage), under two tables that are completely independent of the
//! content-addressed graph:
//!
//! - `projects(id, remote_url, namespace_id, display_name, created_at)`
//! - `project_paths(project_id, local_path)`
//!
//! This must exist in plain SQL and be readable *before* any ASG
//! namespace-scoped operation, because the registry is how we discover
//! which namespace to open.
//!
//! Schema is created on first use (idempotent `CREATE TABLE IF NOT EXISTS`).

use rusqlite::{Connection, Result as SqlResult, params};
use std::path::{Path, PathBuf};

/// A registered project.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    /// Canonical git remote URL, if the project has one. Projects without a
    /// remote are still registerable — they're detected via `.ctxproject`.
    pub remote_url: Option<String>,
    pub namespace_id: String,
    pub display_name: Option<String>,
    pub created_at: String,
    pub local_paths: Vec<String>,
}

/// Open a rusqlite connection to the registry database.
/// The registry tables are bootstrapped inline so callers never need to
/// worry about whether the schema exists yet.
fn open(db_path: &str) -> SqlResult<Connection> {
    let conn = Connection::open(db_path)?;
    bootstrap(&conn)?;
    Ok(conn)
}

/// Create the registry tables if they don't exist yet.
pub fn bootstrap(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS projects (
            id          TEXT PRIMARY KEY,
            remote_url  TEXT UNIQUE,
            namespace_id TEXT NOT NULL,
            display_name TEXT,
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
        );
        CREATE TABLE IF NOT EXISTS project_paths (
            project_id  TEXT NOT NULL,
            local_path  TEXT NOT NULL,
            PRIMARY KEY (project_id, local_path),
            FOREIGN KEY (project_id) REFERENCES projects(id)
        );",
    )
}

// -- Public registry operations --

/// Register a new project in the registry. Returns an error if the id or
/// a non-null `remote_url` is already registered.
pub fn register_project(
    db_path: &str,
    id: &str,
    remote_url: Option<&str>,
    namespace_id: &str,
    display_name: Option<&str>,
    local_path: Option<&str>,
) -> SqlResult<()> {
    let conn = open(db_path)?;
    conn.execute(
        "INSERT INTO projects (id, remote_url, namespace_id, display_name) VALUES (?1, ?2, ?3, ?4)",
        params![id, remote_url, namespace_id, display_name],
    )?;
    if let Some(path) = local_path {
        conn.execute(
            "INSERT OR IGNORE INTO project_paths (project_id, local_path) VALUES (?1, ?2)",
            params![id, path],
        )?;
    }
    Ok(())
}

/// Add a local path binding to an existing project. Idempotent.
pub fn add_local_path(db_path: &str, project_id: &str, local_path: &str) -> SqlResult<()> {
    let conn = open(db_path)?;
    conn.execute(
        "INSERT OR IGNORE INTO project_paths (project_id, local_path) VALUES (?1, ?2)",
        params![project_id, local_path],
    )?;
    Ok(())
}

/// Resolve namespace from a known project id. Returns `None` if not found.
pub fn resolve_by_id(db_path: &str, project_id: &str) -> SqlResult<Option<Project>> {
    let conn = open(db_path)?;
    load_project_by_id(&conn, project_id)
}

/// Resolve namespace from a git remote URL. Returns `None` if not found.
pub fn resolve_by_remote_url(db_path: &str, remote_url: &str) -> SqlResult<Option<Project>> {
    let conn = open(db_path)?;
    load_project_by_remote_url(&conn, remote_url)
}

/// List all registered projects.
pub fn list_projects(db_path: &str) -> SqlResult<Vec<Project>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, remote_url, namespace_id, display_name, created_at FROM projects ORDER BY created_at",
    )?;
    let ids: Vec<(String, Option<String>, String, Option<String>, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<SqlResult<_>>()?;

    let mut projects = Vec::new();
    for (id, remote_url, namespace_id, display_name, created_at) in ids {
        let paths = local_paths_for(&conn, &id)?;
        projects.push(Project {
            id,
            remote_url,
            namespace_id,
            display_name,
            created_at,
            local_paths: paths,
        });
    }
    Ok(projects)
}

// -- Internal helpers --

fn load_project_by_id(conn: &Connection, id: &str) -> SqlResult<Option<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, remote_url, namespace_id, display_name, created_at FROM projects WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    if let Some(row) = rows.next() {
        let (id, remote_url, namespace_id, display_name, created_at) = row?;
        let paths = local_paths_for(conn, &id)?;
        Ok(Some(Project {
            id,
            remote_url,
            namespace_id,
            display_name,
            created_at,
            local_paths: paths,
        }))
    } else {
        Ok(None)
    }
}

fn load_project_by_remote_url(conn: &Connection, remote_url: &str) -> SqlResult<Option<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, remote_url, namespace_id, display_name, created_at FROM projects WHERE remote_url = ?1",
    )?;
    let mut rows = stmt.query_map(params![remote_url], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    if let Some(row) = rows.next() {
        let (id, remote_url, namespace_id, display_name, created_at) = row?;
        let paths = local_paths_for(conn, &id)?;
        Ok(Some(Project {
            id,
            remote_url,
            namespace_id,
            display_name,
            created_at,
            local_paths: paths,
        }))
    } else {
        Ok(None)
    }
}

fn local_paths_for(conn: &Connection, project_id: &str) -> SqlResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT local_path FROM project_paths WHERE project_id = ?1 ORDER BY local_path",
    )?;
    let paths: Vec<String> = stmt
        .query_map(params![project_id], |row| row.get::<_, String>(0))?
        .collect::<SqlResult<_>>()?;
    Ok(paths)
}

// ── Detection chain ──────────────────────────────────────────────────────────

/// Result of the project detection chain.
#[derive(Debug, Clone)]
pub enum DetectResult {
    /// Found a project via `.ctxproject` file.
    FoundByFile {
        project_id: String,
        namespace_id: String,
    },
    /// Found a project via git remote URL lookup.
    FoundByRemote {
        project_id: String,
        namespace_id: String,
        remote_url: String,
    },
    /// Found a project because `cwd` is inside one of its registered
    /// `local_paths`.
    FoundByPath {
        project_id: String,
        namespace_id: String,
        local_path: String,
    },
    /// No project found — caller should warn and fall back to "default".
    NotFound,
    /// Registry is unavailable (non-sqlite backend). Silently use default.
    RegistryUnavailable,
}

/// Run the project detection chain from a given working directory.
///
/// Priority order (stop at first hit):
///
/// 1. `.ctxproject` file in `cwd` or any parent — read project ID, resolve
///    namespace from the registry.
/// 2. Git remote URL lookup in the registry.
/// 3. Longest-prefix match against registered `local_paths`.
/// 4. Return `NotFound`.
///
/// Step 3 exists because `register_project` has always stored `local_paths`
/// while detection never consulted them, so a repo with no remote and no
/// `.ctxproject` marker stayed undetectable however it was registered.
/// Longest-prefix wins so a nested checkout beats its parent.
///
/// `db_path` is `None` for memory/postgres backends, in which case detection
/// is skipped entirely (`RegistryUnavailable`).
pub fn detect_project(cwd: &Path, db_path: Option<&str>) -> DetectResult {
    let Some(db) = db_path else {
        return DetectResult::RegistryUnavailable;
    };

    // Step 1: .ctxproject file walk
    if let Some(project_id) = find_ctxproject_file(cwd) {
        match resolve_by_id(db, &project_id) {
            Ok(Some(p)) => {
                return DetectResult::FoundByFile {
                    project_id: p.id,
                    namespace_id: p.namespace_id,
                };
            }
            Ok(None) => {
                // .ctxproject points to an unknown project — warn, fall through
                // to remote URL lookup before giving up.
            }
            Err(_) => {
                return DetectResult::RegistryUnavailable;
            }
        }
    }

    // Step 2: git remote URL lookup
    if let Some(remote_url) = read_git_remote(cwd) {
        match resolve_by_remote_url(db, &remote_url) {
            Ok(Some(p)) => {
                return DetectResult::FoundByRemote {
                    project_id: p.id,
                    namespace_id: p.namespace_id,
                    remote_url,
                };
            }
            Ok(None) => {}
            Err(_) => {
                return DetectResult::RegistryUnavailable;
            }
        }
    }

    // Step 3: longest registered local_path that contains `cwd`.
    //
    // A query error here means "nothing registered by path" — most often the
    // `project_paths` table does not exist yet because no project has been
    // registered — not "the registry is down". Reporting RegistryUnavailable
    // would turn a normal first-run detection into a hard failure, so this
    // falls through to NotFound.
    if let Ok(Some((p, local_path))) = resolve_by_local_path(db, cwd) {
        return DetectResult::FoundByPath {
            project_id: p.id,
            namespace_id: p.namespace_id,
            local_path,
        };
    }

    DetectResult::NotFound
}

/// The project whose registered `local_path` is the longest prefix of `cwd`.
///
/// Compared on path components, not raw strings, so `/a/repo-two` cannot match
/// a registration for `/a/repo`.
fn resolve_by_local_path(db_path: &str, cwd: &Path) -> SqlResult<Option<(Project, String)>> {
    let conn = rusqlite::Connection::open(db_path)?;
    let mut stmt = conn.prepare("SELECT project_id, local_path FROM project_paths")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;

    let mut best: Option<(String, String, usize)> = None;
    for row in rows {
        let (project_id, local_path) = row?;
        let candidate = Path::new(&local_path);
        if !cwd.starts_with(candidate) {
            continue;
        }
        let depth = candidate.components().count();
        if best.as_ref().map(|(_, _, d)| depth > *d).unwrap_or(true) {
            best = Some((project_id, local_path, depth));
        }
    }

    let Some((project_id, local_path, _)) = best else {
        return Ok(None);
    };
    Ok(resolve_by_id(db_path, &project_id)?.map(|p| (p, local_path)))
}

/// Walk `start` and its parents looking for a `.ctxproject` file.
/// Returns the project ID (first non-empty, non-whitespace line) or `None`.
fn find_ctxproject_file(start: &Path) -> Option<String> {
    let mut dir: Option<&Path> = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(".ctxproject");
        if candidate.is_file() {
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                let id = content
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim().to_string());
                if let Some(id) = id {
                    if !id.is_empty() {
                        return Some(id);
                    }
                }
            }
        }
        dir = d.parent();
    }
    None
}

/// Run `git remote get-url origin` (or `git ls-remote --get-url`) in `dir`
/// and return the canonical remote URL, or `None` if not inside a git repo
/// or no remote is configured.
pub fn read_git_remote(dir: &Path) -> Option<String> {
    // Try `git remote get-url origin` first (most common).
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        let url = raw.trim().to_string();
        if !url.is_empty() {
            return Some(normalize_remote_url(&url));
        }
    }
    // Fallback: `git ls-remote --get-url origin`
    let output = std::process::Command::new("git")
        .args(["ls-remote", "--get-url", "origin"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        let url = raw.trim().to_string();
        // git ls-remote --get-url returns the arg itself when there is no remote
        if !url.is_empty() && url != "origin" {
            return Some(normalize_remote_url(&url));
        }
    }
    None
}

/// Strip trailing `.git` suffix and trailing slashes so that
/// `https://github.com/user/repo.git` and `https://github.com/user/repo`
/// map to the same canonical form. Public so the HTTP layer normalizes
/// user-supplied remote URLs the same way detection normalizes git's.
pub fn normalize_remote_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    trimmed.strip_suffix(".git").unwrap_or(trimmed).to_string()
}

/// Detect the current git branch name from the working directory.
/// Returns `None` if not inside a git repo or the HEAD is detached.
pub fn read_git_branch(dir: &Path) -> Option<String> {
    // First try worktree / detached-head safe approach via symbolic-ref.
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        let branch = raw.trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }
    // Detached HEAD: try to get a descriptive label via rev-parse.
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        let sha = raw.trim().to_string();
        if !sha.is_empty() {
            return Some(format!("detached-{}", sha));
        }
    }
    None
}

/// Sanitize a git branch name for use as an ASG branch component.
/// - Strip leading `refs/heads/`
/// - Replace characters invalid in a namespace-qualified branch name with `-`
/// - Collapse multiple consecutive `-` into one
/// - Strip leading/trailing `-`
pub fn sanitize_branch_name(raw: &str) -> String {
    let stripped = raw.strip_prefix("refs/heads/").unwrap_or(raw);
    let replaced: String = stripped
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Collapse runs of '-'
    let mut out = String::with_capacity(replaced.len());
    let mut last_was_dash = false;
    for c in replaced.chars() {
        if c == '-' {
            if !last_was_dash {
                out.push(c);
            }
            last_was_dash = true;
        } else {
            out.push(c);
            last_was_dash = false;
        }
    }
    // Strip leading/trailing '-'
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "work".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Write a `.ctxproject` file at `repo_root` containing `project_id`.
pub fn write_ctxproject_file(repo_root: &Path, project_id: &str) -> std::io::Result<()> {
    let path = repo_root.join(".ctxproject");
    std::fs::write(&path, format!("{}\n", project_id))
}

/// Find the git repository root starting from `start`.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;
    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        let path = raw.trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn tmp_db() -> (tempfile::TempDir, String) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db").to_string_lossy().to_string();
        (dir, path)
    }

    #[test]
    fn register_and_resolve_by_id() {
        let (_dir, db) = tmp_db();
        register_project(
            &db,
            "proj-1",
            Some("https://github.com/user/repo"),
            "user-repo",
            None,
            Some("/home/user/repo"),
        )
        .unwrap();
        let p = resolve_by_id(&db, "proj-1").unwrap().unwrap();
        assert_eq!(p.namespace_id, "user-repo");
        assert_eq!(
            p.remote_url.as_deref(),
            Some("https://github.com/user/repo")
        );
        assert_eq!(p.local_paths, vec!["/home/user/repo"]);
    }

    #[test]
    fn register_and_resolve_by_remote_url() {
        let (_dir, db) = tmp_db();
        register_project(
            &db,
            "proj-2",
            Some("https://github.com/user/repo2"),
            "user-repo2",
            Some("My Repo"),
            Some("/home/user/repo2"),
        )
        .unwrap();
        let p = resolve_by_remote_url(&db, "https://github.com/user/repo2")
            .unwrap()
            .unwrap();
        assert_eq!(p.id, "proj-2");
        assert_eq!(p.display_name, Some("My Repo".to_string()));
    }

    #[test]
    fn duplicate_remote_url_is_rejected() {
        let (_dir, db) = tmp_db();
        register_project(
            &db,
            "p1",
            Some("https://example.com/repo"),
            "ns1",
            None,
            Some("/a"),
        )
        .unwrap();
        let err = register_project(
            &db,
            "p2",
            Some("https://example.com/repo"),
            "ns2",
            None,
            Some("/b"),
        );
        assert!(err.is_err(), "duplicate remote_url should fail");
    }

    #[test]
    fn list_projects_returns_all() {
        let (_dir, db) = tmp_db();
        register_project(
            &db,
            "p1",
            Some("https://github.com/u/r1"),
            "ns1",
            None,
            Some("/r1"),
        )
        .unwrap();
        register_project(
            &db,
            "p2",
            Some("https://github.com/u/r2"),
            "ns2",
            None,
            Some("/r2"),
        )
        .unwrap();
        let projects = list_projects(&db).unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn add_local_path_is_idempotent() {
        let (_dir, db) = tmp_db();
        register_project(
            &db,
            "p1",
            Some("https://github.com/u/r"),
            "ns1",
            None,
            Some("/r"),
        )
        .unwrap();
        add_local_path(&db, "p1", "/r").unwrap(); // duplicate
        add_local_path(&db, "p1", "/r2").unwrap();
        let p = resolve_by_id(&db, "p1").unwrap().unwrap();
        assert_eq!(p.local_paths.len(), 2);
    }

    #[test]
    fn ctxproject_file_is_found_in_parent() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("sub/dir");
        std::fs::create_dir_all(&nested).unwrap();
        write_ctxproject_file(dir.path(), "my-project-id").unwrap();
        let result = find_ctxproject_file(&nested);
        assert_eq!(result, Some("my-project-id".to_string()));
    }

    #[test]
    fn sanitize_branch_name_handles_common_patterns() {
        assert_eq!(sanitize_branch_name("main"), "main");
        assert_eq!(
            sanitize_branch_name("feature/my-feature"),
            "feature-my-feature"
        );
        assert_eq!(sanitize_branch_name("refs/heads/fix/bug-42"), "fix-bug-42");
        assert_eq!(sanitize_branch_name("user@branch"), "user-branch");
        assert_eq!(sanitize_branch_name("---"), "work"); // all dashes → fallback
    }

    #[test]
    fn normalize_remote_url_strips_git_suffix() {
        assert_eq!(
            normalize_remote_url("https://github.com/user/repo.git"),
            "https://github.com/user/repo"
        );
        assert_eq!(
            normalize_remote_url("https://github.com/user/repo"),
            "https://github.com/user/repo"
        );
        assert_eq!(
            normalize_remote_url("https://github.com/user/repo/"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn detect_project_returns_not_found_when_empty() {
        let dir = tempdir().unwrap();
        let (_dbdir, db) = tmp_db();
        // bootstrap the db so it's valid
        let conn = rusqlite::Connection::open(&db).unwrap();
        bootstrap(&conn).unwrap();
        let result = detect_project(dir.path(), Some(&db));
        assert!(matches!(result, DetectResult::NotFound));
    }

    #[test]
    fn detect_project_finds_by_ctxproject_file() {
        let dir = tempdir().unwrap();
        let (_dbdir, db) = tmp_db();
        register_project(
            &db,
            "test-proj",
            Some("https://github.com/u/r"),
            "test-ns",
            None,
            Some(dir.path().to_str().unwrap()),
        )
        .unwrap();
        write_ctxproject_file(dir.path(), "test-proj").unwrap();
        let result = detect_project(dir.path(), Some(&db));
        assert!(
            matches!(result, DetectResult::FoundByFile { ref namespace_id, .. } if namespace_id == "test-ns")
        );
    }
}
