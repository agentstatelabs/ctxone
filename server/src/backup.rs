//! SQLite snapshot / rolling backup support for the hub.
//!
//! Why this exists: on 2026-04-28 a stray `rm ctxone.db` against a live
//! lens-mode hub silently destroyed the project's plan store. The hub
//! kept its open inode for ~10 minutes, then a restart picked up an
//! empty replacement file and the WAL was gone with the orphaned inode.
//! No backup, no recovery.
//!
//! This module gives the hub three primitives:
//!
//! - `snapshot_now(src, suffix)` — copy a live SQLite db to
//!   `<src>.bak.<suffix>` via `VACUUM INTO`. Safe to call against a
//!   running hub (SQLite serializes writes around it).
//! - `prune(src, keep)` — delete all but the most recent `keep`
//!   snapshots for `src`. Snapshots are sorted by mtime.
//! - `iso_utc_compact()` — produce a filename-safe ISO-8601 UTC
//!   timestamp like `20260428T204812Z`.
//!
//! Failures here NEVER abort the hub. A failed snapshot logs a WARN
//! and returns; the hub keeps serving. Backup mishaps must not take
//! down the primary.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tracing::{info, warn};

/// Filename-safe UTC timestamp: `YYYYMMDDTHHMMSSZ`.
///
/// Avoids colons (Windows-hostile) and dashes-in-time (greppable).
pub fn iso_utc_compact() -> String {
    // We lean on chrono if it's already a workspace dep; otherwise
    // hand-roll from SystemTime to keep this module dep-light.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Convert epoch seconds → UTC components without pulling chrono.
    // Algorithm: classic civil-from-days (Howard Hinnant).
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let h = (secs_of_day / 3600) as u32;
    let m = ((secs_of_day % 3600) / 60) as u32;
    let s = (secs_of_day % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if mo <= 2 { y + 1 } else { y };

    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", y, mo, d, h, m, s)
}

/// Build the snapshot path for a given source db and timestamp suffix.
pub fn snapshot_path(src: &str, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.bak.{}", src, suffix))
}

/// Snapshot `src` into `<src>.bak.<suffix>` via `VACUUM INTO`.
///
/// Uses a fresh connection so it doesn't disturb the hub's own
/// connection pool. Returns the path written on success.
pub fn snapshot_now(src: &str, suffix: &str) -> Result<PathBuf, String> {
    let dst = snapshot_path(src, suffix);
    if dst.exists() {
        // Suffix collision (same-second restart). Append a counter so
        // we never silently overwrite an existing snapshot.
        let mut n = 1;
        loop {
            let candidate = snapshot_path(src, &format!("{}_{}", suffix, n));
            if !candidate.exists() {
                return vacuum_into(src, &candidate);
            }
            n += 1;
            if n > 99 {
                return Err(format!(
                    "snapshot collision: {} already exists with 99 retries",
                    dst.display()
                ));
            }
        }
    }
    vacuum_into(src, &dst)
}

fn vacuum_into(src: &str, dst: &Path) -> Result<PathBuf, String> {
    let conn = Connection::open(src).map_err(|e| format!("open {}: {}", src, e))?;
    // SQLite requires the destination be passed as a string literal in
    // the SQL — we bind it via parameter substitution to avoid quoting
    // hazards on Windows paths.
    conn.execute("VACUUM INTO ?1", [dst.to_string_lossy().as_ref()])
        .map_err(|e| format!("VACUUM INTO {}: {}", dst.display(), e))?;
    Ok(dst.to_path_buf())
}

/// Delete all but the `keep` most recent snapshots for `src`.
///
/// Snapshots are matched by the prefix `<basename>.bak.` in the same
/// directory as `src`. Returns the number of files deleted.
pub fn prune(src: &str, keep: usize) -> Result<usize, String> {
    let src_path = Path::new(src);
    let dir = src_path.parent().unwrap_or_else(|| Path::new("."));
    let basename = src_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| format!("no filename in {}", src))?;
    let prefix = format!("{}.bak.", basename);

    let mut snaps: Vec<(PathBuf, SystemTime)> = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {}", dir.display(), e))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            snaps.push((entry.path(), mtime));
        }
    }

    if snaps.len() <= keep {
        return Ok(0);
    }

    // Newest first → drop everything past `keep`.
    snaps.sort_by(|a, b| b.1.cmp(&a.1));
    let mut removed = 0;
    for (path, _) in snaps.into_iter().skip(keep) {
        match fs::remove_file(&path) {
            Ok(()) => {
                info!(path = %path.display(), "pruned old snapshot");
                removed += 1;
            }
            Err(e) => warn!(path = %path.display(), error = %e, "could not prune snapshot"),
        }
    }
    Ok(removed)
}

/// Convenience: snapshot then prune. Logs and swallows errors so the
/// hub's lifecycle is never interrupted by backup mishaps.
pub fn snapshot_and_prune(src: &str, keep: usize) {
    let suffix = iso_utc_compact();
    match snapshot_now(src, &suffix) {
        Ok(path) => info!(path = %path.display(), "snapshot written"),
        Err(e) => {
            warn!(src = %src, error = %e, "snapshot failed");
            return;
        }
    }
    match prune(src, keep) {
        Ok(n) if n > 0 => info!(count = n, "pruned old snapshots"),
        Ok(_) => {}
        Err(e) => warn!(src = %src, error = %e, "prune failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        // Include pid + nanos + a per-call counter so parallel tests
        // never collide on the same temp directory.
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "ctxone-backup-test-{}-{}-{}-{}",
            tag,
            std::process::id(),
            nanos,
            n
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn snapshot_creates_file() {
        let dir = tempdir(stringify!(test));
        let db = dir.join("a.db");
        // Seed a tiny db.
        let conn = Connection::open(&db).unwrap();
        conn.execute("CREATE TABLE t(x)", []).unwrap();
        conn.execute("INSERT INTO t VALUES (1)", []).unwrap();
        drop(conn);

        let snap = snapshot_now(db.to_str().unwrap(), "20260428T000000Z").expect("snapshot ok");
        assert!(snap.exists(), "snapshot file should exist");
        assert!(snap.to_string_lossy().ends_with(".bak.20260428T000000Z"));

        // Snapshot is itself a valid db with the row.
        let conn = Connection::open(&snap).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_collision_appends_counter() {
        let dir = tempdir(stringify!(test));
        let db = dir.join("b.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute("CREATE TABLE t(x)", []).unwrap();
        drop(conn);

        let s1 = snapshot_now(db.to_str().unwrap(), "FIXED").unwrap();
        let s2 = snapshot_now(db.to_str().unwrap(), "FIXED").unwrap();
        assert_ne!(s1, s2, "second snapshot should not overwrite first");
        assert!(s2.to_string_lossy().contains("FIXED_1"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_only_newest_k() {
        let dir = tempdir(stringify!(test));
        let db = dir.join("c.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute("CREATE TABLE t(x)", []).unwrap();
        drop(conn);

        // Make 5 snapshots with distinct suffixes.
        for i in 0..5 {
            snapshot_now(db.to_str().unwrap(), &format!("S{}", i)).unwrap();
            // Stagger mtimes by sleeping; coarse but enough for test.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let removed = prune(db.to_str().unwrap(), 2).expect("prune ok");
        assert_eq!(removed, 3);

        // Check only 2 snapshots remain.
        let count = fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .map(|e| e.file_name().to_string_lossy().contains(".bak."))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count, 2);

        let _ = fs::remove_dir_all(&dir);
    }
}
