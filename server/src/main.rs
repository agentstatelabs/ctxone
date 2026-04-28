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

use ctxone_hub::{backup, http, lockfile, memory_tools, migrations};

use std::sync::Arc;

use agentstategraph::Repository;
use agentstategraph_storage::{MemoryStorage, SqliteStorage};
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
    // MCP-mode agent ID. The tool that spawns ctxone-hub (Claude
    // Code, Cursor, Codex, etc.) passes --agent-id <its-name> so
    // every commit made via this MCP connection is attributed to
    // that tool in blame history. Defaults to "ctxone" when unset.
    let mut agent_id: String =
        std::env::var("CTX_AGENT_ID").unwrap_or_else(|_| "ctxone".to_string());

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
                eprintln!("  --http                HTTP REST API server");
                eprintln!("  --lens                Serve Lens web UI at / (requires --http)");
                eprintln!();
                eprintln!("OPTIONS:");
                eprintln!(
                    "  -s, --storage <TYPE>  Storage backend: sqlite (default), memory, or postgres"
                );
                eprintln!(
                    "  -p, --path <PATH>     SQLite database path (default: ./target/ctxone.db)"
                );
                eprintln!(
                    "      --init            Create the sqlite db file if it doesn't exist"
                );
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
            info!(storage = "memory", "Storage: in-memory (ephemeral)");
            Arc::new(Repository::new(Box::new(MemoryStorage::new())))
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
            if init_flag && !path_obj.exists()
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

        // Session registry: load persisted stats from SQLite (if sqlite storage),
        // so token savings survive hub restarts. Falls back to empty on memory/pg.
        let sessions = Arc::new(if storage_type == "sqlite" {
            info!("Loading persisted session stats from db");
            memory_tools::SessionRegistry::load_from_db(&db_path)
        } else {
            memory_tools::SessionRegistry::new()
        });

        let hub_config = http::HubConfig { rate_limit_rpm };

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
                        let mut interval = tokio::time::interval(
                            std::time::Duration::from_secs(30)
                        );
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
                        let mut interval = tokio::time::interval(
                            std::time::Duration::from_secs(backup_interval)
                        );
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
                        info!(interval_secs = watch_interval, "inode-drift watchdog scheduled");
                    }
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
                let result = serve
                    .with_graceful_shutdown(async {
                        tokio::signal::ctrl_c()
                            .await
                            .expect("failed to listen for ctrl-c");
                    })
                    .await;
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

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let service = memory_tools::CtxOneServer::with_agent_id(repo, agent_id.clone())
                    .serve(rmcp::transport::stdio())
                    .await
                    .map_err(|e| {
                        error!(error = %e, "MCP server failed to start");
                        format!("MCP server error: {}", e)
                    })?;

                if let Err(e) = service.waiting().await {
                    warn!(error = %e, "MCP server exited with error");
                }
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
    }

    info!("Hub shut down");
    Ok(())
}
