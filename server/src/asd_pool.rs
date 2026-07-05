//! AsdProcessPool — manages a pool of `asd-serve` child processes, one per
//! registered repo.  Processes are spawned lazily on first use and killed
//! after an idle timeout to reclaim memory.
//!
//! Port discovery: `asd-serve` is launched with `ASD_SERVE_ADDR=127.0.0.1:0`
//! so the OS assigns a free port.  The pool reads the first line matching
//! `"listening on 127.0.0.1:<port>"` from the child's stderr, then stores
//! the resolved base URL `http://127.0.0.1:<port>`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Default idle timeout if the caller passes `None`.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(600); // 10 minutes

/// How often the eviction task wakes up to reap idle processes.
const EVICTION_INTERVAL: Duration = Duration::from_secs(60);

/// How long to wait for `asd-serve` to print its listening port and pass /health.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between /health polls after capturing the port.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

struct AsdEntry {
    /// Resolved base URL, e.g. `http://127.0.0.1:54321`.
    base_url: String,
    /// Child process — dropped here kills it (SIGKILL via tokio).
    _child: Child,
    last_used: Instant,
}

struct PoolInner {
    /// repo name → (db_path, maybe-running entry)
    entries: HashMap<String, (PathBuf, Option<AsdEntry>)>,
    /// Path to the `asd-serve` binary.
    binary: String,
    /// Idle eviction threshold.
    idle_timeout: Duration,
}

// ---------------------------------------------------------------------------
// Public
// ---------------------------------------------------------------------------

/// Thread-safe pool of `asd-serve` processes, keyed by repo name.
#[derive(Clone)]
pub struct AsdProcessPool {
    inner: Arc<Mutex<PoolInner>>,
}

impl AsdProcessPool {
    /// Create a new pool.
    ///
    /// `repos` — list of `(name, db_path)` pairs from config or registry.
    /// `binary` — path to `asd-serve`; `None` → look up on `$PATH`.
    /// `idle_timeout` — kill a process after it has been idle this long;
    /// `None` → [`DEFAULT_IDLE_TIMEOUT`].
    pub fn new(
        repos: Vec<(String, String)>,
        binary: Option<String>,
        idle_timeout: Option<Duration>,
    ) -> Self {
        let entries = repos
            .into_iter()
            .map(|(name, path)| (name, (PathBuf::from(path), None::<AsdEntry>)))
            .collect();

        let pool = Self {
            inner: Arc::new(Mutex::new(PoolInner {
                entries,
                binary: binary.unwrap_or_else(|| "asd-serve".to_string()),
                idle_timeout: idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT),
            })),
        };

        // Background eviction task
        let evict = pool.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(EVICTION_INTERVAL);
            loop {
                ticker.tick().await;
                evict.evict_idle().await;
            }
        });

        pool
    }

    /// Return the base URL (`http://127.0.0.1:<port>`) for the named repo,
    /// spawning `asd-serve` if it isn't already running.
    pub async fn base_url(&self, name: &str) -> Result<String, String> {
        // Fast path — process already live
        {
            let mut guard = self.inner.lock().await;
            if let Some((_, Some(entry))) = guard.entries.get_mut(name) {
                entry.last_used = Instant::now();
                return Ok(entry.base_url.clone());
            }
        }

        // Slow path — spawn (lock released while we do async I/O)
        let (db_path, binary) = {
            let guard = self.inner.lock().await;
            let (db, _) = guard
                .entries
                .get(name)
                .ok_or_else(|| format!("repo '{name}' is not registered in the pool"))?;
            (db.clone(), guard.binary.clone())
        };

        let (child, base_url) = spawn_asd_serve(&binary, name, &db_path).await?;

        {
            let mut guard = self.inner.lock().await;
            if let Some((_, slot)) = guard.entries.get_mut(name) {
                *slot = Some(AsdEntry {
                    base_url: base_url.clone(),
                    _child: child,
                    last_used: Instant::now(),
                });
            }
        }

        Ok(base_url)
    }

    /// Kill any process that has been idle longer than the configured timeout.
    async fn evict_idle(&self) {
        let mut guard = self.inner.lock().await;
        let timeout = guard.idle_timeout;
        for (_, (_, slot)) in guard.entries.iter_mut() {
            if let Some(entry) = slot {
                if entry.last_used.elapsed() > timeout {
                    *slot = None; // Drop kills the child
                }
            }
        }
    }

    /// Add or update a repo.  Any running process for this name is killed so
    /// the next `base_url` call starts fresh against the new db path.
    pub async fn upsert(&self, name: String, db_path: String) {
        let mut guard = self.inner.lock().await;
        guard.entries.insert(name, (PathBuf::from(db_path), None));
    }

    /// Remove a repo from the pool, killing its process if running.
    pub async fn remove(&self, name: &str) {
        let mut guard = self.inner.lock().await;
        guard.entries.remove(name);
    }

    /// Names of all registered repos (running or idle).
    pub async fn repo_names(&self) -> Vec<String> {
        let guard = self.inner.lock().await;
        guard.entries.keys().cloned().collect()
    }

    /// True if a live process is currently running for this repo.
    pub async fn is_running(&self, name: &str) -> bool {
        let guard = self.inner.lock().await;
        guard
            .entries
            .get(name)
            .and_then(|(_, s)| s.as_ref())
            .is_some()
    }
}

// ---------------------------------------------------------------------------
// Spawn helper
// ---------------------------------------------------------------------------

/// Spawn `asd-serve` for a registered repo and return `(Child, base_url)`.
///
/// We ask the kernel to pick the port (`ASD_SERVE_ADDR=127.0.0.1:0`) and
/// read the resolved address back from asd-serve's stderr "listening on
/// host:port" log line — asd-serve sets the format from
/// `listener.local_addr()` after bind, so the port is real even when we
/// passed `:0`. After we have the URL, we poll `/api/v1/health` until the
/// child accepts (asd-serve mounts everything under /api/v1, so bare
/// `/health` 404s through the SPA fallback).
async fn spawn_asd_serve(
    binary: &str,
    name: &str,
    db_path: &std::path::Path,
) -> Result<(Child, String), String> {
    let mut child = Command::new(binary)
        .env("ASD_DB", db_path)
        .env("ASD_SERVE_ADDR", "127.0.0.1:0")
        .env("ASD_NAME", name)
        // tracing-subscriber emits ANSI escapes by default; NO_COLOR=1
        // disables them so our "listening on host:port" parse sees a clean
        // address. (We also strip-tolerantly in extract_listening_addr.)
        .env("NO_COLOR", "1")
        // asd-serve initializes tracing-subscriber with the default fmt
        // writer, which is stdout — not stderr. Pipe both so we don't lose
        // diagnostic output if tracing ever moves; read from stdout for the
        // "listening on …" line.
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn asd-serve: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "asd-serve stdout not captured".to_string())?;
    let mut lines = BufReader::new(stdout).lines();

    let base_url = tokio::time::timeout(SPAWN_TIMEOUT, async {
        // Phase 1: capture the resolved bind address.
        let mut url = None;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(addr) = extract_listening_addr(&line) {
                url = Some(format!("http://{addr}"));
                break;
            }
        }
        let url = url
            .ok_or_else(|| "asd-serve exited before printing its listening address".to_string())?;

        // Phase 2: gate on /api/v1/health so callers never see a not-yet-ready URL.
        let health = format!("{url}/api/v1/health");
        let client = reqwest::Client::new();
        loop {
            match client.get(&health).send().await {
                Ok(r) if r.status().is_success() => return Ok::<_, String>(url),
                _ => tokio::time::sleep(HEALTH_POLL_INTERVAL).await,
            }
        }
    })
    .await
    .map_err(|_| {
        format!(
            "timed out waiting for asd-serve to start ({}s)",
            SPAWN_TIMEOUT.as_secs()
        )
    })??;

    tracing::info!(repo = name, url = %base_url, "asd-serve spawned");

    Ok((child, base_url))
}

/// Extract `host:port` from a tracing-formatted "listening on host:port" line.
/// asd-serve logs this after `listener.local_addr()`, so the port reflects
/// what the OS actually assigned (even when we passed `:0`).
fn extract_listening_addr(line: &str) -> Option<&str> {
    let idx = line.find("listening on ")?;
    let rest = &line[idx + "listening on ".len()..];
    let addr = rest.split_whitespace().next()?;
    // Reject the bind template form: an addr ending in `:0` means we caught
    // an old asd-serve that hadn't been rebuilt with the fix. Force a retry
    // by treating it as no-match — the timeout will fire with a clear error.
    if !addr.contains(':') || addr.ends_with(":0") {
        return None;
    }
    Some(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_real_resolved_port() {
        let line = "  INFO asd_serve: listening on 127.0.0.1:60647";
        assert_eq!(extract_listening_addr(line), Some("127.0.0.1:60647"));
    }

    #[test]
    fn rejects_the_zero_port_template() {
        // Old asd-serve logged this when ASD_SERVE_ADDR=host:0 — meaningless
        // to the pool; treat as no-match so the spawn times out loudly.
        let line = "  INFO asd_serve: listening on 127.0.0.1:0";
        assert_eq!(extract_listening_addr(line), None);
    }

    #[test]
    fn ignores_non_listening_lines() {
        let line = "  INFO asd_serve: starting asd-serve db_path=\"/tmp/x\"";
        assert_eq!(extract_listening_addr(line), None);
    }
}
