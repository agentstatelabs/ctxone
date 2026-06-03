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

/// Reserve a free port by binding to `127.0.0.1:0`, reading the resolved
/// port, then closing the listener immediately. The kernel may reuse the
/// same port for the next bind from our child within milliseconds — racy
/// against unrelated processes but acceptable for a local dev pool.
fn reserve_local_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("could not reserve a local port: {e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| format!("could not read reserved port: {e}"))
}

/// Spawn `asd-serve` for a registered repo and return `(Child, base_url)`.
///
/// asd-serve takes its db via `ASD_DB`, its bind address via
/// `ASD_SERVE_ADDR`, and logs its bind string literally — so it can't tell
/// us back which port the OS chose when we pass `:0`. We reserve a free
/// port locally (bind+drop), pass it explicitly, and then poll /health
/// until the child accepts. The reserve-and-pass race is acceptable for a
/// dev pool; a stricter design would patch asd-serve to log
/// `listener.local_addr()` after bind.
async fn spawn_asd_serve(
    binary: &str,
    name: &str,
    db_path: &std::path::Path,
) -> Result<(Child, String), String> {
    let port = reserve_local_port()?;
    let addr = format!("127.0.0.1:{port}");
    let base_url = format!("http://{addr}");

    let child = Command::new(binary)
        .env("ASD_DB", db_path)
        .env("ASD_SERVE_ADDR", &addr)
        .env("ASD_NAME", name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn asd-serve: {e}"))?;

    // Poll /api/v1/health until the child accepts. asd-serve mounts every
    // route under /api/v1, including health — using a bare /health hits the
    // SPA fallback and 404s. Hammer until 200 or the timeout fires.
    let base_url = tokio::time::timeout(SPAWN_TIMEOUT, async {
        let health = format!("{base_url}/api/v1/health");
        let client = reqwest::Client::new();
        loop {
            match client.get(&health).send().await {
                Ok(r) if r.status().is_success() => return Ok::<_, String>(base_url),
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
