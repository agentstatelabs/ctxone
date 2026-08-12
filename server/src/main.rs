//! CtxOne Hub — MCP + HTTP server for AI agent memory.
//!
//! Wraps AgentStateGraph with higher-level memory operations and token tracking.
//!
//! Run as MCP server (stdio):  ctxone-hub
//! Run as HTTP server:         ctxone-hub --http
//! Options:                    ctxone-hub --storage memory
//!                             ctxone-hub --path /data/ctxone.db
//!
//! Default sqlite path is `./target/ctxone.db` — that's the natural
//! ephemeral zone for cargo workspaces. Production deployments should
//! always pass --path explicitly (e.g. /var/lib/ctxone/db).
//!
//! Logging is controlled via the `RUST_LOG` env var (see `tracing-subscriber`
//! docs). Default level is `info`. Examples:
//!     RUST_LOG=debug ctxone-hub --http
//!     RUST_LOG=ctxone_hub=trace ctxone-hub --http
//! All logs go to stderr so they never corrupt the MCP stdio JSON stream.

use ctxone_hub::{asd_pool, backup, http, lockfile, memory_tools, migrations};

use std::sync::Arc;

use agentstategraph::Repository;
use agentstategraph_storage::SqliteStorage;
use rmcp::ServiceExt;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt};

fn init_tracing(mcp_stdio: bool) {
    // Parse RUST_LOG, defaulting to "info" if unset.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = fmt::layer()
        // Always write to stderr. In MCP stdio mode, stdout is the
        // JSON-RPC channel and must never be polluted by log output.
        .with_writer(std::io::stderr)
        // Quieter format for MCP stdio (minimize noise to the parent
        // process), richer format for HTTP mode.
        .with_target(!mcp_stdio)
        .with_level(true)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}

// Bring registry into scope for the init above
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Resolve when the process is asked to stop, by either Ctrl-C or SIGTERM.
///
/// SIGTERM matters more than Ctrl-C in practice: it is what `launchctl
/// bootout`, the systemd unit, `docker stop` and a plain `kill` all send. This
/// previously awaited `ctrl_c()` alone, so every one of those paths killed the
/// hub before the post-shutdown session flush could run, silently losing
/// everything written since the last 30s periodic flush.
///
/// Returns on whichever signal arrives first. Errors installing a handler are
/// logged and that arm is then parked forever rather than aborting shutdown —
/// losing one signal source is survivable, panicking in the shutdown path is
/// not.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(error = %e, "failed to install Ctrl-C handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                error!(error = %e, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    // No SIGTERM on Windows; Ctrl-C is the only stop signal there.
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("shutdown: received Ctrl-C"),
        _ = terminate => info!("shutdown: received SIGTERM"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let mut storage_type = "sqlite";
    // Default lives under target/ so it shares the cargo "ephemeral
    // build artifacts" zone — one less foot-gun for devs who instinctively
    // `rm` files in the repo root. Production setups always pass --path.
    let mut db_path = "./target/ctxone.db".to_string();
    let mut database_url = String::new();
    let mut tenant_id = "default".to_string();
    let mut http_mode = false;
    let mut lens_mode = false;
    let mut http_port: u16 = 3001;
    // Refuse to silently create a fresh sqlite db when the file is
    // missing — operators must opt in with --init. Prevents "ran the
    // hub with the wrong --path and got an empty graph" disasters.
    let mut init_flag = false;
    // Default production rate limit: 600 req/min per peer IP.
    // Overridable via --rate-limit-rpm or CTXONE_RATE_LIMIT_RPM.
    // 0 disables rate limiting.
    let mut rate_limit_rpm: u32 = std::env::var("CTXONE_RATE_LIMIT_RPM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    // Named ASD repos: Vec of (name, base_url).
    // Populated from repeated --asd-url name=http://... flags.
    // Also accepts bare URLs (name defaults to "asd") for backwards compat.
    // Example: --asd-url asd=http://localhost:8787 --asd-url ctxone=http://localhost:8788
    let mut asd_repos: Vec<(String, String)> = Vec::new();
    // Pool-managed ASD repos: Vec of (name, db_path).
    // Hub spawns asd-serve on demand, kills after idle timeout.
    // Example: --asd-repo myproject=/path/to/myproject/.asd-state.db
    let mut asd_pool_repos: Vec<(String, String)> = Vec::new();
    // Idle timeout before the pool kills an asd-serve child (seconds).
    // None → AsdProcessPool default (600s).
    let mut asd_idle_timeout_secs: Option<u64> = None;
    // MCP-mode agent ID. The tool that spawns ctxone-hub (Claude
    // Code, Cursor, Codex, etc.) passes --agent-id <its-name> so
    // every commit made via this MCP connection is attributed to
    // that tool in blame history. Defaults to "ctxone" when unset.
    let mut agent_id: String =
        std::env::var("CTX_AGENT_ID").unwrap_or_else(|_| "ctxone".to_string());
    // Optional bearer token guarding the whole HTTP surface (REST + /mcp).
    // When set, non-loopback requests must send `Authorization: Bearer <token>`;
    // loopback peers stay exempt. From --auth-token or CTXONE_AUTH_TOKEN.
    let mut auth_token: Option<String> = std::env::var("CTXONE_AUTH_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    // Extra browser origins allowed to call the API (repeatable
    // --allowed-origin). Same-origin is always allowed; anything else carrying
    // an Origin header is rejected. Seedable from CTXONE_ALLOWED_ORIGINS (comma-
    // or space-separated) for service/env-based configs.
    let mut allowed_origins: Vec<String> = std::env::var("CTXONE_ALLOWED_ORIGINS")
        .ok()
        .map(|s| {
            s.split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // MCP-mode namespace override. When unset, the project detection
    // chain runs from the process cwd at startup (the spawning tool
    // starts us in the project directory, so this Just Works once the
    // repo is registered via `ctx project add`).
    let mut namespace_flag: Option<String> = std::env::var("CTX_NAMESPACE").ok();
    // Path to the `ctx` CLI binary used by POST /api/sessions/sync to
    // re-ingest local Claude Code transcripts. None → resolve "ctx" on PATH.
    // From --ctx-binary or CTXONE_CTX_BINARY.
    let mut ctx_binary: Option<String> = std::env::var("CTXONE_CTX_BINARY")
        .ok()
        .filter(|s| !s.is_empty());
    // Explicit path to the `asd-serve` binary the code-proxy pool spawns.
    // From --asd-serve-binary or CTXONE_ASD_SERVE_BINARY. When unset, the pool
    // path is resolved robustly (see `asd_pool::resolve_asd_serve_binary`) so a
    // launchd-spawned hub with a minimal PATH still finds the Homebrew install.
    let mut asd_serve_binary: Option<String> = std::env::var("CTXONE_ASD_SERVE_BINARY")
        .ok()
        .filter(|s| !s.is_empty());

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--storage" | "-s" => {
                i += 1;
                if i < args.len() {
                    storage_type = match args[i].as_str() {
                        "memory" => "memory",
                        "postgres" | "pg" => "postgres",
                        _ => "sqlite",
                    };
                }
            }
            "--path" | "-p" => {
                i += 1;
                if i < args.len() {
                    db_path = args[i].clone();
                }
            }
            "--database-url" => {
                i += 1;
                if i < args.len() {
                    database_url = args[i].clone();
                }
            }
            "--tenant" => {
                i += 1;
                if i < args.len() {
                    tenant_id = args[i].clone();
                }
            }
            "--http" => {
                http_mode = true;
            }
            "--lens" => {
                lens_mode = true;
            }
            "--init" => {
                init_flag = true;
            }
            "--port" => {
                i += 1;
                if i < args.len() {
                    http_port = args[i].parse().unwrap_or(3001);
                }
            }
            "--rate-limit-rpm" => {
                i += 1;
                if i < args.len() {
                    rate_limit_rpm = args[i].parse().unwrap_or(600);
                }
            }
            "--agent-id" => {
                i += 1;
                if i < args.len() {
                    agent_id = args[i].clone();
                }
            }
            "--auth-token" => {
                i += 1;
                if i < args.len() {
                    auth_token = Some(args[i].clone()).filter(|s| !s.is_empty());
                }
            }
            "--allowed-origin" => {
                i += 1;
                if i < args.len() && !args[i].is_empty() {
                    allowed_origins.push(args[i].clone());
                }
            }
            "--namespace" => {
                i += 1;
                if i < args.len() {
                    namespace_flag = Some(args[i].clone());
                }
            }
            "--ctx-binary" => {
                i += 1;
                if i < args.len() {
                    ctx_binary = Some(args[i].clone()).filter(|s| !s.is_empty());
                }
            }
            "--asd-serve-binary" => {
                i += 1;
                if i < args.len() {
                    asd_serve_binary = Some(args[i].clone()).filter(|s| !s.is_empty());
                }
            }
            "--asd-url" => {
                i += 1;
                if i < args.len() {
                    let val = args[i].clone();
                    // Accept both "name=http://..." and bare "http://..."
                    if let Some((name, url)) = val.split_once('=') {
                        asd_repos.push((name.to_string(), url.to_string()));
                    } else {
                        asd_repos.push(("asd".to_string(), val));
                    }
                }
            }
            // `--asd-path` is the documented name (matches the `path` field in
            // ~/.config/asd/repos.toml). `--asd-repo` is the original spelling,
            // kept as an alias.
            "--asd-path" | "--asd-repo" => {
                let flag = args[i].clone();
                i += 1;
                if i < args.len() {
                    let val = args[i].clone();
                    if let Some((name, path)) = val.split_once('=') {
                        asd_pool_repos.push((name.to_string(), path.to_string()));
                    } else {
                        eprintln!("ctxone-hub: {flag} requires name=path format, got: {val}");
                        std::process::exit(64);
                    }
                }
            }
            "--asd-idle-timeout" => {
                i += 1;
                if i < args.len() {
                    match args[i].parse::<u64>() {
                        Ok(secs) => asd_idle_timeout_secs = Some(secs),
                        Err(_) => {
                            eprintln!(
                                "ctxone-hub: --asd-idle-timeout requires a non-negative integer (seconds), got: {}",
                                args[i]
                            );
                            std::process::exit(64);
                        }
                    }
                }
            }
            "--version" | "-V" => {
                // Diagnostic — must NOT touch storage. Print and exit
                // before any other work. (The 2026-04-28 incident
                // started with a typo'd flag silently falling through
                // to default mode and opening sqlite.)
                println!("ctxone-hub {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--help" | "-h" => {
                // --help output stays as plain eprintln — it's the classic
                // usage-on-stderr contract, not a log line.
                eprintln!("CtxOne Hub v{}", env!("CARGO_PKG_VERSION"));
                eprintln!();
                eprintln!("USAGE:");
                eprintln!("  ctxone-hub [OPTIONS]");
                eprintln!();
                eprintln!("MODES:");
                eprintln!("  (default)             MCP server over stdio (memory tools)");
                eprintln!(
                    "  --http                HTTP REST API + MCP over HTTP at /mcp (Streamable HTTP)"
                );
                eprintln!("  --lens                Serve Lens web UI at / (requires --http)");
                eprintln!();
                eprintln!("  With --http, one process serves MCP + REST + Lens. Point agents at");
                eprintln!(
                    "  http://<host>:<port>/mcp?namespace=<ns> (see `ctx init --transport http`)."
                );
                eprintln!();
                eprintln!("OPTIONS:");
                eprintln!(
                    "  -s, --storage <TYPE>  Storage backend: sqlite (default), memory, or postgres"
                );
                eprintln!(
                    "  -p, --path <PATH>     SQLite database path (default: ./target/ctxone.db)"
                );
                eprintln!("      --init            Create the sqlite db file if it doesn't exist");
                eprintln!(
                    "                        (without --init, missing files exit 66 — guards against typos)"
                );
                eprintln!("      --database-url <URL>  Postgres connection URL");
                eprintln!(
                    "      --tenant <ID>     Tenant ID for multi-tenant Postgres (default: \"default\")"
                );
                eprintln!("      --port <PORT>     HTTP port (default: 3001, requires --http)");
                eprintln!(
                    "      --rate-limit-rpm <N>  Per-IP rate limit (default: 600 req/min; 0 disables)"
                );
                eprintln!(
                    "      --agent-id <NAME>    Agent ID recorded on commits (default: \"ctxone\")"
                );
                eprintln!(
                    "      --auth-token <TOK>   Bearer token guarding REST + /mcp (env: CTXONE_AUTH_TOKEN)."
                );
                eprintln!(
                    "                           Non-loopback requests must send Authorization: Bearer <TOK>;"
                );
                eprintln!(
                    "                           loopback is exempt. Unset = no auth (warns if bound remotely)."
                );
                eprintln!(
                    "      --namespace <NS>     MCP mode: namespace to operate in (default: detect \
                     project from cwd, else \"default\")"
                );
                eprintln!(
                    "      --asd-url <name=URL>  Register an ASD repo with a pre-running server; repeatable."
                );
                eprintln!(
                    "                            Proxies /api/code/<name>/* → <URL>/api/v1/*"
                );
                eprintln!(
                    "                            Bare URL (no name=) defaults to name \"asd\""
                );
                eprintln!(
                    "      --asd-path <name=PATH> Register an ASD repo db for the process pool; repeatable."
                );
                eprintln!(
                    "                            Hub spawns asd-serve on demand, kills after idle timeout."
                );
                eprintln!(
                    "                            Example: --asd-path myproject=/path/.asd-state.db"
                );
                eprintln!(
                    "                            (--asd-repo is accepted as a legacy alias.)"
                );
                eprintln!(
                    "      --asd-serve-binary <PATH>  Path to the asd-serve binary the pool spawns."
                );
                eprintln!(
                    "                            Default: resolved automatically (PATH, next to the"
                );
                eprintln!(
                    "                            hub, or a common install dir). Set this (or"
                );
                eprintln!(
                    "                            CTXONE_ASD_SERVE_BINARY) if the hub runs under a"
                );
                eprintln!(
                    "                            minimal PATH (launchd/systemd) and can't find it."
                );
                eprintln!(
                    "      --asd-idle-timeout <SECS>  Pool idle timeout before killing an asd-serve"
                );
                eprintln!("                            child (default 600).");
                eprintln!(
                    "      --ctx-binary <PATH>  Path to the `ctx` CLI for POST /api/sessions/sync"
                );
                eprintln!(
                    "                            (re-ingests local transcripts; default: `ctx` on PATH)."
                );
                eprintln!("  -h, --help            Print help");
                eprintln!("  -V, --version         Print version and exit");
                eprintln!();
                eprintln!("LOGGING:");
                eprintln!("      RUST_LOG=<level>  info (default), debug, trace, warn, error");
                eprintln!("                        e.g., RUST_LOG=debug ctxone-hub --http");
                eprintln!();
                eprintln!("MCP TOOLS:");
                eprintln!("  remember              Store a fact in agent memory");
                eprintln!("  recall                Retrieve relevant memories (token-budgeted)");
                eprintln!("  context               Load full context for a project");
                eprintln!("  summarize_session     End-of-session knowledge commit");
                eprintln!("  what_changed_since    See changes since a date");
                eprintln!("  why_did_we            Trace decision provenance");
                std::process::exit(0);
            }
            other => {
                // Strict parsing: unknown flags exit EX_USAGE (64) instead
                // of silently falling through to default mode. The earlier
                // permissive behavior was the chain-start in the
                // 2026-04-28 lens-db loss.
                eprintln!("ctxone-hub: unknown argument: {}", other);
                eprintln!("Run `ctxone-hub --help` for usage.");
                std::process::exit(64);
            }
        }
        i += 1;
    }

    // Initialize tracing before doing anything loggable.
    init_tracing(!http_mode);

    info!(version = env!("CARGO_PKG_VERSION"), "CtxOne Hub starting");

    // Auto-discovery: when the user gave no --asd-url / --asd-path flags,
    // populate the pool from ~/.config/asd/repos.toml so `ctxone-hub --http`
    // works without arguments once `asd repo add` has been run. Explicit
    // flags always win.
    if asd_repos.is_empty() && asd_pool_repos.is_empty() {
        if let Some(d) = ctxone_hub::asd_registry::discover() {
            for r in &d.repos {
                asd_pool_repos.push((r.name.clone(), r.path.display().to_string()));
            }
            if !d.repos.is_empty() {
                let names: Vec<&str> = d.repos.iter().map(|r| r.name.as_str()).collect();
                info!(
                    count = d.repos.len(),
                    active = d.active.as_deref().unwrap_or("(none)"),
                    repos = ?names,
                    "auto-discovered asd repos from ~/.config/asd/repos.toml",
                );
            }
        }
    }

    // Check for DATABASE_URL env var as fallback for postgres
    if database_url.is_empty()
        && let Ok(url) = std::env::var("DATABASE_URL")
    {
        database_url = url;
        if storage_type == "sqlite" {
            storage_type = "postgres";
        }
    }

    let repo: Arc<Repository> = match storage_type {
        "memory" => {
            info!(storage = "memory", "Storage: in-memory SQLite (ephemeral)");
            Arc::new(Repository::new(Box::new(
                SqliteStorage::in_memory().expect("in-memory sqlite"),
            )))
        }
        "postgres" => {
            if database_url.is_empty() {
                error!("--database-url or DATABASE_URL required for postgres storage");
                std::process::exit(1);
            }
            info!(
                storage = "postgres",
                tenant = %tenant_id,
                "Storage: postgres"
            );
            let rt = tokio::runtime::Runtime::new()?;
            let storage = rt.block_on(async {
                agentstategraph_storage::PostgresStorage::connect_tenant(&database_url, &tenant_id)
                    .await
            })?;
            Arc::new(Repository::new(Box::new(storage)))
        }
        _ => {
            // Refuse to silently create a fresh db when the operator
            // didn't ask for one. The default is to assume the file
            // should already exist; a missing file usually means a
            // typo'd --path or a mis-launched hub, NOT "please give
            // me a brand new graph". --init opts in. EX_NOINPUT (66).
            let path_obj = std::path::Path::new(&db_path);
            if !path_obj.exists() && !init_flag {
                error!(
                    path = %db_path,
                    "sqlite db not found; pass --init to create a new one, \
                     or --path <PATH> to point at an existing db"
                );
                std::process::exit(66);
            }
            // --init implies "yes, set up the world" — create the
            // parent directory if needed so the default ./target/ path
            // works on a fresh checkout before `cargo build` has run.
            if init_flag
                && !path_obj.exists()
                && let Some(parent) = path_obj.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    error!(parent = %parent.display(), error = %e, "could not create db parent directory");
                    std::process::exit(73); // EX_CANTCREAT
                }
                info!(parent = %parent.display(), "created db parent directory");
            }
            info!(storage = "sqlite", path = %db_path, init = init_flag, "Storage: sqlite");
            // Durability tuning (WAL + synchronous=NORMAL) now lives in
            // SqliteStorage::open itself (agentstategraph-storage >= v0.9.4), so
            // every consumer of the crate gets the faster write path.
            let storage = SqliteStorage::open(&db_path)?;
            Arc::new(Repository::new(Box::new(storage)))
        }
    };

    repo.init()?;

    // Run ASG-level schema migrations first — the substrate's
    // /_meta/schema_version must be current before CTXone starts
    // reading or writing plan/task state.
    //
    // Policy: auto-migrate on UpgradeAvailable / Unversioned; refuse
    // startup on Downgrade or Corrupt. Respect ASG_MIGRATE=prompt|never|auto
    // for operators who want to gate the apply step.
    {
        use agentstategraph_migrate::{
            CheckResult, Registry, RunMode, binary_version, check, exit as asg_exit,
        };
        let asg_registry = Registry::builtin();
        let asg_target = binary_version();
        let policy = std::env::var("ASG_MIGRATE").unwrap_or_else(|_| "auto".into());

        match check(&repo, "main", &asg_target, &asg_registry) {
            Ok(CheckResult::UpToDate { version }) => {
                info!(schema_version = %version, "ASG schema up to date");
            }
            Ok(CheckResult::Downgrade { db, binary }) => {
                error!(
                    db_schema = %db,
                    binary_schema = %binary,
                    "ASG db schema is newer than this binary; refusing to start"
                );
                std::process::exit(asg_exit::DOWNGRADE_REFUSED);
            }
            Ok(CheckResult::Corrupt(msg)) => {
                error!(error = %msg, "ASG /_meta is corrupt; refusing to start");
                std::process::exit(asg_exit::CORRUPT_META);
            }
            Ok(CheckResult::UpgradeAvailable { .. } | CheckResult::Unversioned { .. }) => {
                if policy == "never" {
                    error!(policy = %policy, "ASG migration required but policy=never");
                    std::process::exit(asg_exit::UPGRADE_REQUIRED);
                }
                info!(policy = %policy, "running ASG schema migrations");
                if let Err(e) = asg_registry.run(&repo, "main", &asg_target, RunMode::Apply) {
                    error!(error = %e, "ASG migration failed");
                    std::process::exit(asg_exit::MIGRATION_FAILED);
                }
                info!(schema_version = %asg_target, "ASG schema migrations complete");
            }
            Err(e) => {
                error!(error = %e, "ASG check failed");
                std::process::exit(asg_exit::MIGRATION_FAILED);
            }
        }
    }

    // Run CTXone schema migrations. Refuses to start if the graph was
    // written by a newer Hub binary (prevents silent data corruption on
    // accidental downgrade).
    if let Err(e) = migrations::run_migrations(&repo) {
        error!(error = %e, "migration failed, aborting startup");
        std::process::exit(1);
    }

    // Acquire <db>.lock so a second hub against the same path refuses
    // to start. Must happen before backups so two racing hubs don't
    // both snapshot the same db at the same moment. Skip for memory/
    // postgres backends — those don't have a path to lock against.
    let _lock_guard = if storage_type == "sqlite" {
        match lockfile::acquire(&db_path, env!("CARGO_PKG_VERSION")) {
            Ok(g) => Some(g),
            Err(msg) => {
                error!(error = %msg, "lockfile acquire failed");
                std::process::exit(75); // EX_TEMPFAIL
            }
        }
    } else {
        None
    };

    // Capture the (dev, ino) of the db file at open. The watchdog
    // (HTTP mode only) compares against this every N seconds and
    // logs a WARN if the file gets replaced or unlinked.
    let db_baseline = if storage_type == "sqlite" {
        lockfile::fingerprint(&db_path)
    } else {
        None
    };

    // Startup snapshot: copy the live db to <db>.bak.<utc> before
    // accepting any traffic. Keeps the last K backups (default 5,
    // override CTXONE_BACKUP_KEEP=N). Skip for memory/postgres. A
    // failed snapshot logs a WARN and lets the hub keep going —
    // backups must never block startup.
    let backup_keep: usize = std::env::var("CTXONE_BACKUP_KEEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    if storage_type == "sqlite" && backup_keep > 0 {
        backup::snapshot_and_prune(&db_path, backup_keep);
    }

    // Report AGENTS.md presence so operators can see at a glance
    // whether the Hub is serving pinned agent guidance. This is the
    // disclosure surface for the `ctx agents install` flow — if the
    // file is primed, it shows up here. If it isn't, the line tells
    // the operator exactly how to prime it.
    let agents_paths = repo
        .list_paths("main", "/memory/pinned/ctxone-agents", Some(50))
        .unwrap_or_default();
    let agents_sections = agents_paths
        .iter()
        .filter(|p| p.ends_with("/title"))
        .count();
    if agents_sections > 0 {
        info!(
            sections = agents_sections,
            path = "/memory/pinned/ctxone-agents",
            "AGENTS.md: primed"
        );
    } else {
        info!("AGENTS.md: not primed — run `ctx agents install` to pin the agent guidance");
    }

    if http_mode {
        let addr = format!("0.0.0.0:{}", http_port);
        info!(
            port = http_port,
            addr = %addr,
            rate_limit_rpm,
            "HTTP API listening"
        );
        info!("Try: curl http://localhost:{}/api/health", http_port);

        // Auth posture. The socket binds 0.0.0.0 (all interfaces), so without a
        // token the REST API + /mcp are reachable from the network unauthenticated.
        // Loopback peers are always exempt; a token only gates non-loopback.
        if auth_token.is_some() {
            info!(
                "Bearer auth enabled: non-loopback requests require Authorization: Bearer <token> (loopback exempt)"
            );
            if lens_mode {
                info!(
                    "Lens UI is served, but a remote browser can't send a bearer token — \
                     for remote access tunnel it (ssh -L {port}:localhost:{port} <host>) so \
                     the browser is loopback.",
                    port = http_port
                );
            }
        } else {
            warn!(
                "No auth token set — REST API and /mcp are reachable on ALL interfaces \
                 with no authentication. Set --auth-token / CTXONE_AUTH_TOKEN before \
                 exposing this hub beyond localhost."
            );
        }

        // Session registry: load persisted stats from SQLite (if sqlite storage),
        // so token savings survive hub restarts. Falls back to empty on memory/pg.
        let sessions = Arc::new(if storage_type == "sqlite" {
            info!("Loading persisted session stats from db");
            memory_tools::SessionRegistry::load_from_db(&db_path)
        } else {
            memory_tools::SessionRegistry::new()
        });

        // Loopback base URL the hub hands to `ctx ingest-session --all` for
        // session-sync. The socket binds 0.0.0.0, which is not a valid connect
        // target, so we always dial back over 127.0.0.1 on the same port.
        let self_base_url = format!("http://127.0.0.1:{}", http_port);

        // Background-sweep toggle, shared between the HTTP handlers (which the
        // Lens autosync switch calls) and the sweep loop below. The env var
        // seeds the FIRST run; after that the persisted preference wins, so a
        // toggle in the UI survives a restart. Persisted beside the CLI's own
        // config under ~/.ctxone so wiping the graph db never loses it.
        let autosync_env: u64 = std::env::var("CTXONE_SESSION_SYNC_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        // Persist beside the CLI's config (~/.ctxone). No `dirs` dep in the hub
        // crate, so resolve HOME directly; None disables persistence (the
        // toggle then lives only for the process lifetime).
        let autosync_path = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|h| h.join(".ctxone").join("hub-autosync.json"));
        let autosync = http::Autosync::new(autosync_path, autosync_env);

        // Resolve the `asd-serve` path once, robust to launchd's minimal PATH
        // (the failure mode where a plist-launched hub couldn't find the
        // Homebrew install and every code-proxy call errored "No such file").
        let resolved_asd_serve = asd_pool::resolve_asd_serve_binary(asd_serve_binary.clone());
        match &resolved_asd_serve {
            Some(p) => info!(path = %p, "resolved asd-serve binary for code proxy"),
            None => warn!(
                "could not resolve an asd-serve binary; the code proxy will try 'asd-serve' on \
                 $PATH and fail under a minimal PATH — install asd or pass --asd-serve-binary"
            ),
        }

        let hub_config = http::HubConfig {
            rate_limit_rpm,
            asd_repos: asd_repos.clone(),
            asd_pool_repos: asd_pool_repos.clone(),
            asd_serve_binary: resolved_asd_serve,
            asd_idle_timeout_secs,
            // Serve MCP at /mcp so this one daemon covers MCP + REST + Lens.
            mcp_http: true,
            agent_id: agent_id.clone(),
            auth_token: auth_token.clone(),
            allowed_origins: allowed_origins.clone(),
            ctx_binary: ctx_binary.clone(),
            self_base_url: Some(self_base_url),
            autosync: Some(autosync.clone()),
        };

        // Values for the background session-sweep task (below). Cloned before
        // `hub_config` is moved into the router.
        let sync_ctx_bin = hub_config.ctx_binary.clone();
        let sync_base_url = hub_config.self_base_url.clone();
        let sync_autosync = autosync.clone();

        // Capture db_path for background flush tasks.
        let flush_db_path = if storage_type == "sqlite" {
            Some(db_path.clone())
        } else {
            None
        };

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let app = http::router_with_db_path(
                    repo.clone(),
                    sessions.clone(),
                    hub_config,
                    flush_db_path.clone(),
                    lens_mode,
                );

                // Spawn background flush task: writes session stats to SQLite every 30s.
                if let Some(ref path) = flush_db_path {
                    let sessions_bg = sessions.clone();
                    let path_bg = path.clone();
                    tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_secs(30));
                        interval.tick().await; // skip the immediate first tick
                        loop {
                            interval.tick().await;
                            sessions_bg.flush_to_db(&path_bg);
                        }
                    });
                }

                // Spawn background snapshot task: VACUUM INTO every
                // N seconds (default 1800 = 30min, env
                // CTXONE_BACKUP_INTERVAL_SECS, set to 0 to disable).
                // Errors log WARN; never panic the runtime.
                let backup_interval: u64 = std::env::var("CTXONE_BACKUP_INTERVAL_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1800);
                if let Some(ref path) = flush_db_path
                    && backup_interval > 0
                    && backup_keep > 0
                {
                    let path_bg = path.clone();
                    tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_secs(backup_interval));
                        interval.tick().await; // skip the immediate first tick
                        loop {
                            interval.tick().await;
                            // Run on a blocking thread so VACUUM INTO
                            // doesn't park the async runtime on a slow
                            // disk.
                            let p = path_bg.clone();
                            tokio::task::spawn_blocking(move || {
                                backup::snapshot_and_prune(&p, backup_keep);
                            });
                        }
                    });
                    info!(
                        interval_secs = backup_interval,
                        keep = backup_keep,
                        "background snapshot task scheduled"
                    );
                }

                // Inode-drift watchdog: stat the db every N seconds
                // (default 30, env CTXONE_WATCHDOG_INTERVAL_SECS, 0
                // disables) and warn if the file got replaced or
                // unlinked under us. This is the primary detection
                // for the 2026-04-28 failure mode.
                if let (Some(path), Some(baseline)) = (&flush_db_path, db_baseline) {
                    let watch_interval: u64 = std::env::var("CTXONE_WATCHDOG_INTERVAL_SECS")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(30);
                    if watch_interval > 0 {
                        lockfile::spawn_watchdog(path.clone(), baseline, watch_interval);
                        info!(
                            interval_secs = watch_interval,
                            "inode-drift watchdog scheduled"
                        );
                    }
                }

                // Keep the flat-size (savings-ratio baseline) fresh off the
                // request path: a background task recomputes it every ~20s so no
                // recall/write ever serializes the whole graph (the -32001 cause).
                memory_tools::spawn_flat_size_refresher(repo.clone(), 20);

                // Background session-sweep (auto-sync): the hub can PULL agent
                // transcripts from every source on a schedule. This is **opt-in**
                // and DEFAULT OFF — a fresh hub must not silently ingest a
                // machine's entire agent history (that first cold sweep was slow,
                // timed out, and imported sessions the user may want private).
                //
                // The loop always spawns but is gated on the shared `Autosync`
                // switch, which the Lens toggle flips at runtime (no restart) and
                // persists. It wakes on a fixed cadence and only sweeps when
                // enabled and at least the configured interval has elapsed, so a
                // cadence change takes effect within one probe tick.
                if let Some(base_url) = sync_base_url.clone() {
                    let ctx_bin = sync_ctx_bin.clone().unwrap_or_else(|| "ctx".to_string());
                    let autosync = sync_autosync.clone();
                    tokio::spawn(async move {
                        // Probe often enough that a newly-enabled sweep starts
                        // promptly, but never busier than the floor cadence.
                        let probe = std::time::Duration::from_secs(http::Autosync::MIN_INTERVAL);
                        let mut interval = tokio::time::interval(probe);
                        interval.tick().await; // skip the immediate first tick
                        let mut last_sweep: Option<std::time::Instant> = None;
                        loop {
                            interval.tick().await;
                            if !autosync.enabled() {
                                continue;
                            }
                            let due = last_sweep
                                .is_none_or(|t| t.elapsed().as_secs() >= autosync.interval_secs());
                            if !due {
                                continue;
                            }
                            last_sweep = Some(std::time::Instant::now());
                            // Awaiting to completion before the next tick means
                            // sweeps never overlap even if one runs long.
                            match http::run_session_sync(&ctx_bin, &base_url).await {
                                Ok((sessions, turns, tokens, elapsed_ms)) => info!(
                                    sessions,
                                    turns, tokens, elapsed_ms, "background session sweep complete"
                                ),
                                Err((_code, msg)) => {
                                    warn!(error = %msg, "background session sweep failed")
                                }
                            }
                        }
                    });
                    info!(
                        enabled = sync_autosync.enabled(),
                        interval_secs = sync_autosync.interval_secs(),
                        "background session sweep loop spawned (runtime-toggled)"
                    );
                }

                let listener = tokio::net::TcpListener::bind(&addr).await?;
                // `into_make_service_with_connect_info::<SocketAddr>()` attaches
                // the peer IP to each request so the rate limiter's
                // PeerIpKeyExtractor can actually see client addresses.
                let serve = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                );
                // Graceful shutdown on Ctrl-C / SIGTERM: flush sessions before exit.
                let result = serve.with_graceful_shutdown(shutdown_signal()).await;
                if let Some(ref path) = flush_db_path {
                    info!("Flushing session stats to db on shutdown");
                    sessions.flush_to_db(path);
                }
                if let Err(e) = result {
                    error!(error = %e, "HTTP server error");
                    return Err::<(), Box<dyn std::error::Error>>(e.into());
                }
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
    } else {
        info!(
            transport = "stdio",
            agent_id = %agent_id,
            "MCP server waiting for client (tools: remember, recall, prime, context, \
             summarize_session, what_changed_since, why_did_we)"
        );

        // Resolve the namespace this MCP session operates in, and whether it
        // was chosen *deliberately*: an explicit --namespace / CTX_NAMESPACE, or
        // a successful project detection (.ctxproject walk-up, then git remote).
        // Falling through to "default" is NOT deliberate — write tools refuse a
        // fallback default so data can't silently pile up in the shared
        // workspace (an explicit `default` is still honored).
        let (namespace, namespace_explicit): (String, bool) = match namespace_flag.clone() {
            Some(ns) => (ns, true),
            None => {
                let detected = if storage_type == "sqlite" {
                    std::env::current_dir().ok().and_then(|cwd| {
                        match ctxone_hub::project::detect_project(&cwd, Some(&db_path)) {
                            ctxone_hub::project::DetectResult::FoundByFile {
                                project_id,
                                namespace_id,
                            } => {
                                info!(project = %project_id, namespace = %namespace_id, via = "ctxproject", "project detected");
                                Some(namespace_id)
                            }
                            ctxone_hub::project::DetectResult::FoundByRemote {
                                project_id,
                                namespace_id,
                                ..
                            } => {
                                info!(project = %project_id, namespace = %namespace_id, via = "git-remote", "project detected");
                                Some(namespace_id)
                            }
                            _ => None,
                        }
                    })
                } else {
                    None
                };
                match detected {
                    Some(ns) => (ns, true),
                    None => {
                        warn!(
                            "no workspace derived for this directory — MCP writes to `default` \
                             will be refused; set CTX_NAMESPACE=<ns> or run `ctx project add`"
                        );
                        (agentstategraph_core::Namespace::DEFAULT.to_string(), false)
                    }
                }
            }
        };

        // Fork the repository into the resolved namespace so every tool
        // call in this session is scoped without further plumbing. init()
        // is idempotent and guarantees the namespace has a main branch.
        let repo = if namespace != agentstategraph_core::Namespace::DEFAULT {
            let ns = agentstategraph_core::Namespace::new(namespace.as_str())
                .map_err(|e| format!("invalid namespace '{}': {}", namespace, e))?;
            let forked = repo.fork_namespace(ns);
            forked.init()?;
            Arc::new(forked)
        } else {
            repo
        };
        info!(namespace = %namespace, "MCP session namespace");

        // Branch mirroring: inside a project namespace, the session's
        // default ref is the sanitized current git branch (auto-created
        // from main, raw name recorded as metadata). Detached HEADs are
        // skipped so mirroring never manufactures per-commit branches.
        let mut default_ref = "main".to_string();
        if namespace != agentstategraph_core::Namespace::DEFAULT
            && let Ok(cwd) = std::env::current_dir()
            && let Some(raw_branch) = ctxone_hub::project::read_git_branch(&cwd)
            && !raw_branch.starts_with("detached-")
        {
            let mirrored = ctxone_hub::project::sanitize_branch_name(&raw_branch);
            if mirrored != "main" {
                match repo.branch(&mirrored, "main") {
                    Ok(_) => {
                        let opts = agentstategraph::CommitOptions::new(
                            &agent_id,
                            agentstategraph_core::IntentCategory::Custom("Observe".to_string()),
                            format!("branch {} mirrors git branch {}", mirrored, raw_branch),
                        );
                        let _ = repo.set_json(
                            "main",
                            &format!("/ctxone/branches/{}/git_branch", mirrored),
                            &serde_json::json!(raw_branch),
                            opts,
                        );
                        info!(branch = %mirrored, git_branch = %raw_branch, "mirrored branch created");
                    }
                    Err(agentstategraph::RepoError::BranchAlreadyExists(_)) => {}
                    Err(e) => {
                        error!(error = %e, branch = %mirrored, "failed to ensure mirrored branch");
                    }
                }
            }
            default_ref = mirrored;
            info!(default_ref = %default_ref, "session default ref (git mirror)");
        }

        // Session-stats persistence for the stdio MCP server (session-metrics
        // t-014). Historically the stdio server wrote `recall`/`remember`
        // savings into a private `SessionStats::new()` that lived in no
        // registry and was never flushed — so every byte of MCP-side savings
        // evaporated on process exit. Here we:
        //   1. resolve a stable session id (CTX_SESSION > project namespace >
        //      agent id > "default"),
        //   2. LOAD any persisted row for that id so we accumulate rather than
        //      clobber (the flush upsert OVERWRITES tokens_saved, so a fresh
        //      zeroed session would otherwise reset a prior total), and
        //   3. share that one `Arc<SessionStats>` between a `SessionRegistry`
        //      and the `CtxOneServer`, then flush it periodically and on exit
        //      using the same machinery the HTTP hub uses.
        // Skipped entirely for memory/postgres — no db_path to persist to.
        // Attribution key for live recall + LLM-usage counters. We prefer an
        // explicit CTX_SESSION, then the host agent's own session id when it
        // exposes one (CLAUDE_SESSION_ID today; add siblings as other agents
        // gain the convention). Matching the agent's session id is what makes a
        // live session's recall savings land on the SAME row that transcript
        // ingest later records LLM usage against — without it, recall pools on a
        // generic "default"/agent_id row and per-session savings never appear.
        let mcp_session_id: String = ["CTX_SESSION", "CLAUDE_SESSION_ID"]
            .iter()
            .find_map(|k| {
                std::env::var(k)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| {
                if namespace != agentstategraph_core::Namespace::DEFAULT {
                    namespace.clone()
                } else if !agent_id.is_empty() {
                    agent_id.clone()
                } else {
                    "default".to_string()
                }
            });
        info!(session = %mcp_session_id, "MCP session-stats id (savings accounting)");

        // (registry, shared session Arc, db_path). None on memory/postgres.
        let session_persistence: Option<(
            Arc<memory_tools::SessionRegistry>,
            Arc<memory_tools::SessionStats>,
            String,
        )> = if storage_type == "sqlite" {
            info!("Loading persisted session stats from db (stdio)");
            let registry = Arc::new(memory_tools::SessionRegistry::load_from_db(&db_path));
            // get_or_create returns the loaded Arc when the row existed (seeded
            // with prior totals), else a fresh zeroed one — either way the
            // server and registry now share this exact Arc.
            let session = registry.get_or_create(&mcp_session_id);
            Some((registry, session, db_path.clone()))
        } else {
            None
        };

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let mut ctx_server = memory_tools::CtxOneServer::with_agent_id_and_repos(
                    repo,
                    agent_id.clone(),
                    asd_repos.clone(),
                )
                .with_namespace(namespace.clone())
                .with_namespace_explicit(namespace_explicit)
                .with_default_ref(default_ref.clone());
                // Share the persisted session Arc so MCP savings land in the
                // same counters the registry flushes.
                if let Some((_, ref session, _)) = session_persistence {
                    ctx_server = ctx_server.with_session(session.clone());
                }
                // Attach pool if any --asd-repo flags were given
                if !asd_pool_repos.is_empty() {
                    let pool = std::sync::Arc::new(ctxone_hub::asd_pool::AsdProcessPool::new(
                        asd_pool_repos.clone(),
                        None,
                        asd_idle_timeout_secs.map(std::time::Duration::from_secs),
                    ));
                    ctx_server = ctx_server.with_pool(pool);
                }

                // Periodic flush: mirror the HTTP hub's 30s background task so
                // long-lived stdio sessions persist savings even before exit.
                if let Some((ref registry, _, ref path)) = session_persistence {
                    let registry_bg = registry.clone();
                    let path_bg = path.clone();
                    tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_secs(30));
                        interval.tick().await; // skip the immediate first tick
                        loop {
                            interval.tick().await;
                            registry_bg.flush_to_db(&path_bg);
                        }
                    });
                }

                let service = ctx_server
                    .serve(rmcp::transport::stdio())
                    .await
                    .map_err(|e| {
                        error!(error = %e, "MCP server failed to start");
                        format!("MCP server error: {}", e)
                    })?;

                if let Err(e) = service.waiting().await {
                    warn!(error = %e, "MCP server exited with error");
                }

                // Graceful shutdown flush: persist final savings before exit.
                if let Some((ref registry, _, ref path)) = session_persistence {
                    info!("Flushing session stats to db on stdio shutdown");
                    registry.flush_to_db(path);
                }
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
    }

    info!("Hub shut down");
    Ok(())
}
