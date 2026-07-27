//! Parse Cursor chat sessions into source-neutral [`Turn`]s.
//!
//! Cursor stores chats in a SQLite db (`state.vscdb`), not one file per
//! session, under a `cursorDiskKV(key TEXT, value BLOB)` table:
//!
//! - `composerData:<composerId>` — one per session. Carries `name`,
//!   `createdAt`, `lastUpdatedAt`, and `fullConversationHeadersOnly`: an
//!   **ordered** list of `{ bubbleId, type }`.
//! - `bubbleId:<composerId>:<bubbleId>` — one per message. `type` 1 = user,
//!   2 = assistant; `text` is the content; `tokenCount` is
//!   `{ inputTokens, outputTokens }`.
//!
//! A turn is a user bubble plus the assistant bubbles that follow it, in the
//! order the header list gives — same reconstruction as Codex/Gemini.
//!
//! The format is undocumented and version-specific, so every field is read
//! defensively: a shape we don't recognise yields fewer turns, never a panic
//! or a wrong attribution. The working directory is not in this global db
//! (Cursor keeps it in per-workspace dbs), so Cursor sessions route to the
//! default workspace until that mapping is added.

use crate::ingest::{Turn, TurnTokens};
use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;

/// Message roles, from a bubble's `type`.
const TYPE_USER: i64 = 1;
const TYPE_ASSISTANT: i64 = 2;

/// One session's identity, discovered without reading its bubbles.
pub struct CursorSession {
    pub composer_id: String,
    pub name: Option<String>,
    /// `createdAt`, epoch ms — used only to order sessions on discovery.
    pub created_at: i64,
    /// `lastUpdatedAt`, epoch ms — the per-composer change-detector for
    /// incremental sync (mtime can't work: all composers share one db file).
    pub last_updated_at: i64,
}

/// List every composer (session) in the db, oldest first. Best-effort: a db
/// that can't be opened or has no chat table yields an empty list.
pub fn list_sessions(db_path: &Path) -> Vec<CursorSession> {
    let Ok(conn) = open_readonly(db_path) else {
        return vec![];
    };
    // A half-open **range** on the key, not `LIKE`/`CAST` — this is the whole
    // performance of discovery. `cursorDiskKV(key TEXT UNIQUE, value BLOB)` has
    // ~100k rows (one per chat *bubble*) totalling several GB, since the big
    // JSON lives inline in `value`. `WHERE CAST(key AS TEXT) LIKE
    // 'composerData:%'` cannot use the key index — the CAST defeats it — so it
    // FULL-SCANS the table, reading every value page: ~4GB off disk, ~150s on a
    // cold cache (and it looked instant from the `sqlite3` CLI only because the
    // file was already warm in RAM). The range `key >= 'composerData:' AND key
    // < 'composerData;'` (';' is ':' + 1) is sargable: EXPLAIN QUERY PLAN shows
    // SEARCH USING INDEX, touching only the ~106 composer rows. Identical
    // result set, effectively instant.
    //
    // `value` is still read as `CAST(... AS TEXT)`: though it is JSON text it
    // carries BLOB affinity, so reading it as `Vec<u8>` errored every row and
    // `flatten()` silently dropped them all. (Exact `key = ?` lookups below use
    // the index directly.)
    let mut stmt = match conn.prepare(
        "SELECT CAST(key AS TEXT), CAST(value AS TEXT) FROM cursorDiskKV \
         WHERE key >= 'composerData:' AND key < 'composerData;'",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    let Ok(rows) = rows else {
        return vec![];
    };

    let mut out = vec![];
    for (key, json) in rows.flatten() {
        let Some(id) = key.strip_prefix("composerData:") else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&json) else {
            continue;
        };
        out.push(CursorSession {
            composer_id: id.to_string(),
            name: v.get("name").and_then(|n| n.as_str()).map(str::to_string),
            created_at: v.get("createdAt").and_then(|c| c.as_i64()).unwrap_or(0),
            last_updated_at: v.get("lastUpdatedAt").and_then(|c| c.as_i64()).unwrap_or(0),
        });
    }
    out.sort_by_key(|s| s.created_at);
    out
}

/// Reconstruct one session's turns by walking its ordered bubble headers.
pub fn parse_session(db_path: &Path, composer_id: &str) -> Vec<Turn> {
    let Ok(conn) = open_readonly(db_path) else {
        return vec![];
    };

    // The composer holds the ordered list of bubble ids.
    let composer = match read_kv(&conn, &format!("composerData:{composer_id}")) {
        Some(v) => v,
        None => return vec![],
    };
    let headers = composer
        .get("fullConversationHeadersOnly")
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default();

    let mut turns: Vec<Turn> = vec![];
    let mut cur: Option<Turn> = None;

    for header in &headers {
        let Some(bubble_id) = header.get("bubbleId").and_then(|b| b.as_str()) else {
            continue;
        };
        let bubble = match read_kv(&conn, &format!("bubbleId:{composer_id}:{bubble_id}")) {
            Some(b) => b,
            None => continue,
        };
        // Prefer the bubble's own type; fall back to the header's.
        let btype = bubble
            .get("type")
            .or_else(|| header.get("type"))
            .and_then(|t| t.as_i64())
            .unwrap_or(0);
        let text = bubble.get("text").and_then(|t| t.as_str()).unwrap_or("");
        let tokens = tokens_from(bubble.get("tokenCount"));

        match btype {
            TYPE_USER => {
                if let Some(t) = cur.take() {
                    turns.push(t);
                }
                cur = Some(Turn {
                    user_text: text.to_string(),
                    assistant_text: String::new(),
                    tool_calls: vec![],
                    tool_calls_raw: vec![],
                    tokens,
                    model: String::new(),
                    timestamp: String::new(), // bubbles carry no reliable ts
                    cwd: None,
                    git_branches: vec![],
                    git_commit: None,
                });
            }
            TYPE_ASSISTANT => {
                let t = cur.get_or_insert_with(empty_turn);
                if !text.is_empty() {
                    if !t.assistant_text.is_empty() {
                        t.assistant_text.push_str("\n\n");
                    }
                    t.assistant_text.push_str(text);
                }
                t.tokens.add(&tokens);
            }
            _ => {} // unknown bubble kind: skip rather than misattribute
        }
    }

    if let Some(t) = cur.take() {
        turns.push(t);
    }
    turns
}

fn empty_turn() -> Turn {
    Turn {
        user_text: String::new(),
        assistant_text: String::new(),
        tool_calls: vec![],
        tool_calls_raw: vec![],
        tokens: TurnTokens::default(),
        model: String::new(),
        timestamp: String::new(),
        cwd: None,
        git_branches: vec![],
        git_commit: None,
    }
}

/// Cursor `{ inputTokens, outputTokens }` → the normalised token fields.
fn tokens_from(v: Option<&Value>) -> TurnTokens {
    let Some(v) = v else {
        return TurnTokens::default();
    };
    let g = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    TurnTokens {
        input: g("inputTokens"),
        output: g("outputTokens"),
        ..Default::default()
    }
}

fn read_kv(conn: &Connection, key: &str) -> Option<Value> {
    // `CAST(value AS TEXT)` for the same affinity reason as list_sessions.
    let json: String = conn
        .query_row(
            "SELECT CAST(value AS TEXT) FROM cursorDiskKV WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .ok()?;
    serde_json::from_str(&json).ok()
}

/// Open the chat db for a read-only scan without mutating the user's data.
///
/// Cursor's db is WAL mode. A plain `SQLITE_OPEN_READ_ONLY` open cannot read a
/// WAL database's contents — reading the WAL needs write access to the `-shm`
/// file — so it silently returns an EMPTY snapshot (0 rows, no error). And
/// `immutable=1`, which works from the `sqlite3` CLI, returned 0 rows through
/// rusqlite's bundled SQLite (a build/version difference).
///
/// What works reliably: open read-write so SQLite reads the WAL properly, then
/// `PRAGMA query_only = ON` before any statement, which blocks every write
/// including the checkpoint SQLite might otherwise run. Ingest never issues a
/// write anyway; this makes it impossible. Safe because Cursor is a
/// single-writer app the user is not running during a `ctx` sync.
fn open_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal Cursor db: one composer, two bubbles.
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.vscdb");
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB)",
            [],
        )
        .unwrap();
        let put = |k: &str, v: Value| {
            conn.execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![k, serde_json::to_vec(&v).unwrap()],
            )
            .unwrap();
        };
        put(
            "composerData:c1",
            serde_json::json!({
                "name": "Test chat", "createdAt": 1000,
                "fullConversationHeadersOnly": [
                    { "bubbleId": "b1", "type": 1 },
                    { "bubbleId": "b2", "type": 2 },
                    { "bubbleId": "b3", "type": 1 },
                ]
            }),
        );
        put(
            "bubbleId:c1:b1",
            serde_json::json!({ "type": 1, "text": "hello",
            "tokenCount": { "inputTokens": 10, "outputTokens": 0 } }),
        );
        put(
            "bubbleId:c1:b2",
            serde_json::json!({ "type": 2, "text": "hi there",
            "tokenCount": { "inputTokens": 0, "outputTokens": 5 } }),
        );
        put(
            "bubbleId:c1:b3",
            serde_json::json!({ "type": 1, "text": "thanks" }),
        );
        (dir, path)
    }

    #[test]
    fn lists_and_parses_a_session() {
        let (_d, path) = fixture();

        let sessions = list_sessions(&path);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].composer_id, "c1");
        assert_eq!(sessions[0].name.as_deref(), Some("Test chat"));

        let turns = parse_session(&path, "c1");
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].user_text, "hello");
        assert_eq!(turns[0].assistant_text, "hi there");
        assert_eq!(turns[0].tokens.input, 10);
        assert_eq!(turns[0].tokens.output, 5);
        assert_eq!(turns[1].user_text, "thanks");
    }

    #[test]
    fn tolerates_a_missing_db() {
        assert!(list_sessions(Path::new("/nonexistent/state.vscdb")).is_empty());
        assert!(parse_session(Path::new("/nonexistent/state.vscdb"), "c1").is_empty());
    }
}
