//! Process-exclusive lockfile + inode-drift watchdog for the sqlite db.
//!
//! Two failure modes from the 2026-04-28 incident this defends against:
//!
//! 1. **Two hubs on one db.** Trivially possible today — start a second
//!    hub against the same --path, both happily write through their own
//!    connection pools, and you discover the corruption hours later.
//!    Solution: drop a `<db>.lock` containing `{pid, started_at, version}`
//!    on startup. If the file exists and the PID is alive, refuse.
//!
//! 2. **Live db file replaced/unlinked under us.** SQLite + Unix unlink
//!    semantics mean the hub keeps writing to a deleted inode for as
//!    long as it stays open. By restart time, the WAL is gone and so
//!    is the data. Solution: a background watchdog stats the db path
//!    every 30s and compares (dev, inode) to what we saw on open. On
//!    drift, log WARN — operator gets ~30s of warning instead of
//!    silent loss until the next restart.
//!
//! Both pieces are best-effort. The lockfile uses an atomic
//! create-if-missing open so two simultaneous hubs racing for the
//! same db get a deterministic loser. The watchdog never panics.
//!
//! **Windows note:** `pid_is_alive` falls back to a `tasklist`-based
//! check and the inode watchdog is a no-op (Windows inodes are not
//! stable across renames the same way Unix inodes are).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{info, warn};

/// Lock filename for a given db path: `<db>.lock`.
pub fn lock_path(db_path: &str) -> PathBuf {
    PathBuf::from(format!("{}.lock", db_path))
}

/// RAII guard. Holds the lockfile for the lifetime of the value;
/// removes it on Drop. The `released` atomic lets external shutdown
/// hooks (signal handlers) flip the guard to "already cleaned up" so
/// Drop doesn't double-remove.
#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
    released: Arc<AtomicBool>,
}

impl LockGuard {
    /// Mark the lock as already released. Subsequent Drop is a no-op.
    /// Intended for the graceful-shutdown path where we explicitly
    /// remove the file before tearing down the guard.
    pub fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            let _ = fs::remove_file(&self.path);
            info!(lock = %self.path.display(), "lockfile released");
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Try to acquire `<db>.lock`. Returns Err with a human message if
/// another live hub already holds it. Stale locks (PID dead) are
/// reclaimed with a warning.
pub fn acquire(db_path: &str, hub_version: &str) -> Result<LockGuard, String> {
    let path = lock_path(db_path);

    // If a lockfile is already there, decide whether it's live or stale.
    if path.exists() {
        match read_pid(&path) {
            Some(pid) if pid_is_alive(pid) => {
                return Err(format!(
                    "database is already locked by ctxone-hub pid {} (lockfile: {}); \
                     refusing to start a second hub against the same db",
                    pid,
                    path.display()
                ));
            }
            Some(pid) => {
                warn!(
                    pid = pid,
                    lock = %path.display(),
                    "stale lockfile (owner not running) — reclaiming"
                );
                let _ = fs::remove_file(&path);
            }
            None => {
                warn!(lock = %path.display(), "unreadable lockfile — reclaiming");
                let _ = fs::remove_file(&path);
            }
        }
    }

    // Atomic create — if two hubs race here, exactly one wins.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("could not create lockfile {}: {}", path.display(), e))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!(
        "{{\"pid\":{},\"started_at_unix\":{},\"hub_version\":\"{}\"}}\n",
        std::process::id(),
        now,
        hub_version
    );
    file.write_all(body.as_bytes())
        .map_err(|e| format!("could not write lockfile body: {}", e))?;
    drop(file);

    info!(lock = %path.display(), "lockfile acquired");

    Ok(LockGuard {
        path,
        released: Arc::new(AtomicBool::new(false)),
    })
}

/// Read the PID line from a lockfile. Returns None if missing/garbled.
fn read_pid(path: &Path) -> Option<u32> {
    let body = fs::read_to_string(path).ok()?;
    // Cheap parse — we wrote the JSON ourselves so we don't reach for
    // serde. Look for the substring `"pid":N`.
    let after = body.split("\"pid\":").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Returns true if `pid` is a running process.
///
/// Unix: `kill -0 <pid>` — zero cost, no signal sent.
/// Windows: `tasklist /FI "PID eq <pid>"` — heavier but correct.
fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output();
        match out {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout.contains(&pid.to_string())
            }
            Err(_) => false,
        }
    }
}

// -- Inode-drift watchdog --

/// (device, inode) pair — what `stat` returns on Unix.
/// On Windows this is always `(0, 0)`; the watchdog is a no-op there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbFingerprint {
    pub dev: u64,
    pub ino: u64,
}

/// Capture the current (dev, ino) of a path. None if the file is
/// missing or unreadable. On Windows always returns `Some((0, 0))`.
pub fn fingerprint(path: &str) -> Option<DbFingerprint> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(path).ok()?;
        Some(DbFingerprint {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        // Windows: file exists check only; inode tracking not supported.
        fs::metadata(path).ok()?;
        Some(DbFingerprint { dev: 0, ino: 0 })
    }
}

/// Spawn the inode-drift watchdog as a tokio task.
///
/// On Unix: stats `db_path` every `interval_secs` and compares against
/// `baseline`. On drift (file replaced or missing), logs a single WARN.
/// On Windows: no-op (inode semantics differ; the file-missing check
/// still fires via `fingerprint` returning None).
pub fn spawn_watchdog(db_path: String, baseline: DbFingerprint, interval_secs: u64) {
    if interval_secs == 0 {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        interval.tick().await; // skip immediate first tick
        let mut last = baseline;
        loop {
            interval.tick().await;
            match fingerprint(&db_path) {
                None => {
                    tracing::error!(
                        path = %db_path,
                        "database file is missing — likely deleted under a running hub. \
                         Writes still hit the orphaned inode but will be lost on restart. \
                         Restore from <db>.bak.<utc> snapshot."
                    );
                    // No baseline update — keep complaining until it
                    // reappears so the operator can't miss it.
                }
                Some(now) if now != last => {
                    #[cfg(unix)]
                    warn!(
                        path = %db_path,
                        old_dev = last.dev, old_ino = last.ino,
                        new_dev = now.dev, new_ino = now.ino,
                        "database file replaced — current process is still writing to the OLD inode. \
                         Restart the hub to attach to the new file."
                    );
                    last = now;
                }
                Some(_) => {} // no drift, quiet path
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn unique_db_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "ctxone-lock-test-{}-{}-{}.db",
            tag,
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn acquire_creates_lockfile() {
        let db = unique_db_path("create");
        let _ = fs::remove_file(lock_path(db.to_str().unwrap()));
        let guard = acquire(db.to_str().unwrap(), "test").expect("acquire ok");
        assert!(lock_path(db.to_str().unwrap()).exists());
        drop(guard);
        assert!(
            !lock_path(db.to_str().unwrap()).exists(),
            "drop should remove lockfile"
        );
    }

    #[test]
    fn second_acquire_with_live_pid_fails() {
        let db = unique_db_path("conflict");
        let _ = fs::remove_file(lock_path(db.to_str().unwrap()));
        let _g1 = acquire(db.to_str().unwrap(), "test").expect("first acquire ok");
        let err = acquire(db.to_str().unwrap(), "test").expect_err("second acquire should fail");
        assert!(err.contains("already locked"), "got: {}", err);
    }

    #[test]
    fn stale_lock_is_reclaimed() {
        let db = unique_db_path("stale");
        let lock = lock_path(db.to_str().unwrap());
        let _ = fs::remove_file(&lock);
        // Plant a lockfile with a PID that's almost certainly dead
        // (PID 1 is init/launchd which IS alive — pick something
        // unlikely. We use a very high PID that exceeds typical
        // pid_max on macOS and Linux).
        let mut f = File::create(&lock).unwrap();
        writeln!(
            f,
            "{{\"pid\":2147483646,\"started_at_unix\":0,\"hub_version\":\"x\"}}"
        )
        .unwrap();
        drop(f);
        let guard = acquire(db.to_str().unwrap(), "test").expect("stale lock should be reclaimed");
        // Sanity — guard now owns the (rewritten) lockfile.
        let pid = read_pid(&lock).unwrap();
        assert_eq!(pid, std::process::id());
        drop(guard);
    }

    #[test]
    fn fingerprint_changes_when_file_replaced() {
        let db = unique_db_path("fp");
        fs::write(&db, b"v1").unwrap();
        let fp1 = fingerprint(db.to_str().unwrap()).expect("fp1");
        // Atomic replace via rename produces a new inode on Unix.
        // On Windows dev/ino are both 0 so fp1 == fp2; skip the assert there.
        let tmp = unique_db_path("fp_tmp");
        fs::write(&tmp, b"v2").unwrap();
        fs::rename(&tmp, &db).unwrap();
        let fp2 = fingerprint(db.to_str().unwrap()).expect("fp2");
        #[cfg(unix)]
        assert_ne!(fp1.ino, fp2.ino, "rename should change inode");
        #[cfg(not(unix))]
        let _ = (fp1, fp2); // no-op on Windows
        let _ = fs::remove_file(&db);
    }
}
