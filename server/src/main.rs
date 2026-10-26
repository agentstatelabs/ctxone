//! CtxOne Hub — MCP + HTTP server for AI agent memory.
//!
//! Wraps AgentStateGraph with higher-level memory operations and token tracking.
//!
//! Run as MCP server (stdio):  ctxone-hub
//! Run as HTTP server:         ctxone-hub --http
//! Options:                    ctxone-hub --storage memory
//!                             ctxone-hub --path /data/ctxone.db
//! Many tenants, one database: ctxone-hub --http --tenants-file <PATH> --database-url <URL>
//!
//! Default sqlite path is `./target/ctxone.db` — that's the natural
//! ephemeral zone for cargo workspaces. Production deployments should
//! always pass --path explicitly (e.g. /var/lib/ctxone/db).
//!
//! Logging is controlled via the `RUST_LOG` env var. Default level is
//! `info`. Examples:
//!     RUST_LOG=debug ctxone-hub --http
//!     RUST_LOG=ctxone_hub=trace ctxone-hub --http
//! All logs go to stderr so they never corrupt the MCP stdio JSON stream:
//! text on a terminal, one JSON object per line otherwise
//! (`OXTEL_LOG_FORMAT=text|json|ecs|none` chooses). Telemetry is oxtel's;
//! nothing is exported unless the hub is built with the `otlp` feature and
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set.

use ctxone_hub::{
    asd_pool, backup, http, lockfile, memory_tools, migrations, tenant, tenant_cache,
};

use std::sync::Arc;

use agentstategraph::Repository;
use agentstategraph_storage::SqliteStorage;
use rmcp::ServiceExt;
use tracing::{error, info, warn};

/// Set up logs, traces and metrics with oxtel. Log lines go to stderr, never
/// stdout, which is the MCP stdio channel: text on a terminal, one JSON
/// object per line otherwise (`OXTEL_LOG_FORMAT` picks another), at
/// `RUST_LOG` or `info`. Nothing is sent anywhere unless the server was built
/// with the `otlp` feature and `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Keep the
/// guard until `main` returns: dropping it sends what's pending.
fn init_telemetry() -> Result<oxtel::Guard, oxtel::Error> {
    oxtel::builder()
        .with_default_service_name("ctxone-hub")
        .with_resource_attribute(oxtel::KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION"),
        ))
        .install()
}

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
    // The Postgres tenant of a single-tenant hub. None → "default".
    let mut tenant_id: Option<String> = None;
    // Most connections the Postgres pool holds, however many tenants share it.
    // From --pg-pool-size or CTXONE_PG_POOL_SIZE.
    let mut pg_pool_size: usize = match std::env::var("CTXONE_PG_POOL_SIZE") {
        Ok(v) if !v.is_empty() => parse_pool_size(&v),
        _ => agentstategraph_storage::postgres::DEFAULT_POOL_SIZE,
    };
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
    // Bearer tokens from a file, re-read while the hub runs so they rotate
    // without a restart. From --auth-token-file or CTXONE_AUTH_TOKEN_FILE.
    let mut auth_token_file: Option<String> = std::env::var("CTXONE_AUTH_TOKEN_FILE")
        .ok()
        .filter(|s| !s.is_empty());
    // Multi-tenant mode: tenants and the hashes of their tokens, re-read while
    // the hub runs. From --tenants-file or CTXONE_TENANTS_FILE.
    let mut tenants_file: Option<String> = std::env::var("CTXONE_TENANTS_FILE")
        .ok()
        .filter(|s| !s.is_empty());
    // Make loopback peers send the token too (a proxy or sidecar on 127.0.0.1
    // in front of the hub). From --require-auth-on-loopback or
    // CTXONE_REQUIRE_AUTH_ON_LOOPBACK.
    let mut require_auth_on_loopback = env_truthy("CTXONE_REQUIRE_AUTH_ON_LOOPBACK");
    // Allow browser requests only from --allowed-origin, never because Origin
    // matches Host. From --strict-origin or CTXONE_STRICT_ORIGIN.
    let mut strict_origin = env_truthy("CTXONE_STRICT_ORIGIN");
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
                    tenant_id = Some(args[i].clone());
                }
            }
            "--pg-pool-size" => {
                i += 1;
                if i < args.len() {
                    pg_pool_size = parse_pool_size(&args[i]);
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
            "--auth-token-file" => {
                i += 1;
                if i < args.len() {
                    auth_token_file = Some(args[i].clone()).filter(|s| !s.is_empty());
                }
            }
            "--tenants-file" => {
                i += 1;
                if i < args.len() {
                    tenants_file = Some(args[i].clone()).filter(|s| !s.is_empty());
                }
            }
            "--require-auth-on-loopback" => {
                require_auth_on_loopback = true;
            }
            "--strict-origin" => {
                strict_origin = true;
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
                    "      --tenant <ID>     The Postgres tenant this hub serves (default: \"default\")"
                );
                eprintln!(
                    "      --pg-pool-size <N>  Most Postgres connections the hub holds (default: 32;"
                );
                eprintln!(
                    "                        env: CTXONE_PG_POOL_SIZE). A multi-tenant hub's tenants share them."
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
                    "      --auth-token-file <PATH>  Bearer tokens, one per line (env: CTXONE_AUTH_TOKEN_FILE)."
                );
                eprintln!(
                    "                           Re-read every second, so tokens rotate without a restart."
                );
                eprintln!(
                    "      --tenants-file <PATH>  Multi-tenant mode: `<tenant> <sha256 of a token>` lines"
                );
                eprintln!(
                    "                           (env: CTXONE_TENANTS_FILE), re-read every second. Each"
                );
                eprintln!(
                    "                           request's token picks its tenant. Needs --http and"
                );
                eprintln!(
                    "                           Postgres (or --storage memory to try it); turns off what"
                );
                eprintln!(
                    "                           acts on this machine (ASD repos, session sync, backups)."
                );
                eprintln!(
                    "      --require-auth-on-loopback  Loopback peers must send the token too, for a proxy or"
                );
                eprintln!(
                    "                           sidecar on 127.0.0.1 (env: CTXONE_REQUIRE_AUTH_ON_LOOPBACK)."
                );
                eprintln!(
                    "      --allowed-origin <ORIGIN>  Browser origin allowed to call the API; repeatable"
                );
                eprintln!(
                    "                           (env: CTXONE_ALLOWED_ORIGINS). Same-origin is allowed too,"
                );
                eprintln!(
                    "                           unless --strict-origin (env: CTXONE_STRICT_ORIGIN) is set."
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

    // Auth settings that can't work are refused here, before anything listens.
    if auth_token.is_some() && auth_token_file.is_some() {
        eprintln!(
            "ctxone-hub: use --auth-token or --auth-token-file, not both (a token file can hold \
             several tokens)"
        );
        std::process::exit(64);
    }
    let auth_token_file: Option<http::AuthTokenFile> = match auth_token_file {
        Some(path) => match http::AuthTokenFile::open(&path) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("ctxone-hub: can't read --auth-token-file {path}: {e}");
                std::process::exit(66);
            }
        },
        None => None,
    };
    // Check for DATABASE_URL env var as fallback for postgres
    if database_url.is_empty()
        && let Ok(url) = std::env::var("DATABASE_URL")
    {
        database_url = url;
        if storage_type == "sqlite" {
            storage_type = "postgres";
        }
    }

    // Multi-tenant mode: each request's token picks its tenant, every
    // tenant's data is on one database, and nothing acts on this machine,
    // since the tenants are elsewhere (docs/design/MULTI_TENANT_HUB.md).
    // Settings that can't apply are refused rather than ignored.
    let tenants = match &tenants_file {
        None => None,
        Some(path) => {
            if auth_token.is_some() || auth_token_file.is_some() {
                refuse_tenants_file("replaces --auth-token and --auth-token-file; use one");
            }
            if !http_mode {
                refuse_tenants_file("needs --http: tenants reach the hub over HTTP");
            }
            if storage_type == "sqlite" {
                refuse_tenants_file(
                    "needs Postgres (--storage postgres and --database-url, or DATABASE_URL), or \
                     --storage memory to try it",
                );
            }
            if tenant_id.is_some() {
                refuse_tenants_file("replaces --tenant: each request's token picks its tenant");
            }
            if lens_mode {
                refuse_tenants_file("can't serve Lens yet: Lens can't send a tenant's token");
            }
            if !asd_repos.is_empty()
                || !asd_pool_repos.is_empty()
                || asd_serve_binary.is_some()
                || asd_idle_timeout_secs.is_some()
            {
                refuse_tenants_file(
                    "turns off ASD code intelligence, which is this machine's: drop --asd-url, \
                     --asd-path, --asd-serve-binary and --asd-idle-timeout (and \
                     CTXONE_ASD_SERVE_BINARY)",
                );
            }
            if ctx_binary.is_some() {
                refuse_tenants_file(
                    "turns off session sync and import, which read this machine's transcripts: \
                     drop --ctx-binary (and CTXONE_CTX_BINARY)",
                );
            }
            match tenant::TenantsFile::open(path) {
                Ok(file) => Some((path.clone(), file)),
                Err(e) => {
                    eprintln!("ctxone-hub: --tenants-file: {e}");
                    std::process::exit(66);
                }
            }
        }
    };
    if require_auth_on_loopback
        && auth_token.is_none()
        && auth_token_file.is_none()
        && tenants.is_none()
    {
        eprintln!(
            "ctxone-hub: --require-auth-on-loopback needs a token (--auth-token or \
             --auth-token-file)"
        );
        std::process::exit(64);
    }

    // Initialize telemetry before doing anything loggable.
    let _telemetry = init_telemetry()?;

    info!(version = env!("CARGO_PKG_VERSION"), "CtxOne Hub starting");

    if let Some((path, file)) = tenants {
        return serve_tenants(TenantsRun {
            path,
            file,
            database_url: (storage_type == "postgres").then_some(database_url),
            pg_pool_size,
            port: http_port,
            rate_limit_rpm,
            agent_id,
            allowed_origins,
            strict_origin,
        });
    }

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

    let tenant_id = tenant_id.unwrap_or_else(|| "default".to_string());

    // Epoch seal enforcement is ON (ASG's default) as of agentstategraph
    // v1.1.0. It used to be disabled everywhere: the epoch store was global, so
    // a sealed epoch in one workspace hard-rejected ref updates in EVERY other
    // workspace, and completing a plan in one project could block writes across
    // all of them. v1.1.0 scopes an epoch to its owning workspace, so a seal
    // now binds only the workspace that made it — which is what the guard was
    // always meant to do.
    // A Postgres hub keeps its project registry and session statistics in
    // `ctxone_*` tables on the storage's own pool; captured here because the
    // storage itself is boxed into the repository.
    let mut postgres_tables: Option<ctxone_hub::hub_store::PgStore> = None;
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
            // Never the URL itself: it may carry a password.
            let tls = ctxone_hub::postgres_url::TlsIntent::of(&database_url);
            info!(
                storage = "postgres",
                tenant = %tenant_id,
                tls = tls.label(),
                "Storage: postgres"
            );
            if let Some(warning) = tls.warning() {
                warn!("{warning}");
            }
            let rt = tokio::runtime::Runtime::new()?;
            let storage = rt.block_on(async {
                agentstategraph_storage::PostgresStorage::connect_tenant_with_pool_size(
                    &database_url,
                    &tenant_id,
                    pg_pool_size,
                )
                .await
            })?;
            match ctxone_hub::hub_store::PgStore::new(&storage) {
                Ok(tables) => postgres_tables = Some(tables),
                Err(e) => {
                    error!(error = %e, "could not set up the hub's Postgres tables");
                    std::process::exit(1);
                }
            }
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

    // Where the project registry and session statistics live: the SQLite file,
    // or the `ctxone_*` tables of a Postgres hub. None in memory mode.
    let hub_store: Option<ctxone_hub::hub_store::HubStore> = match storage_type {
        "sqlite" => Some(ctxone_hub::hub_store::HubStore::Sqlite(db_path.clone())),
        "postgres" => postgres_tables
            .clone()
            .map(ctxone_hub::hub_store::HubStore::Postgres),
        _ => None,
    };

    // Run ASG-level schema migrations first — the substrate's
    // /_meta/schema_version must be current before CTXone starts
    // reading or writing plan/task state.
    //
    // Policy: auto-migrate on UpgradeAvailable / Unversioned; refuse
    // startup on Downgrade or Corrupt. ASG_MIGRATE=never refuses to migrate.
    // A multi-tenant Hub runs the same step as each tenant opens.
    match migrations::migrate_asg(&repo) {
        Ok(migrations::AsgSchema::UpToDate(version)) => {
            info!(schema_version = %version, "ASG schema up to date");
        }
        Ok(migrations::AsgSchema::Migrated(version)) => {
            info!(schema_version = %version, "ASG schema migrations complete");
        }
        Err(e) => {
            error!(error = %e, "refusing to start");
            std::process::exit(e.exit_code());
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
        if auth_token.is_some() || auth_token_file.is_some() {
            if require_auth_on_loopback {
                info!(
                    "Bearer auth enabled: every request requires Authorization: Bearer <token>, \
                     loopback included"
                );
            } else {
                info!(
                    "Bearer auth enabled: non-loopback requests require Authorization: Bearer <token> (loopback exempt)"
                );
            }
            if let Some(file) = &auth_token_file {
                info!(
                    path = %file.path().display(),
                    "Bearer tokens read from a file, re-read every second (rotate by editing it)"
                );
            }
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
        log_strict_origin(strict_origin, &allowed_origins);

        // Session registry: load persisted stats (SQLite file or Postgres
        // tables), so token savings survive hub restarts. Empty in memory mode.
        let sessions = Arc::new(match &hub_store {
            Some(store) => {
                info!("Loading persisted session stats");
                memory_tools::SessionRegistry::load(store)
            }
            None => memory_tools::SessionRegistry::new(),
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
            auth_token_file: auth_token_file.clone(),
            require_auth_on_loopback,
            allowed_origins: allowed_origins.clone(),
            strict_origin,
            ctx_binary: ctx_binary.clone(),
            self_base_url: Some(self_base_url),
            autosync: Some(autosync.clone()),
            postgres_tables: postgres_tables.clone(),
        };

        // Values for the background session-sweep task (below). Cloned before
        // `hub_config` is moved into the router.
        let sync_ctx_bin = hub_config.ctx_binary.clone();
        let sync_base_url = hub_config.self_base_url.clone();
        let sync_autosync = autosync.clone();
        let sync_auth = http::AuthTokens::new(auth_token.clone(), auth_token_file.clone());

        // The SQLite file, for backups and the integrity baseline (session
        // stats flush through `hub_store`).
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

                // Spawn background flush task: writes session stats every 30s.
                if let Some(ref store) = hub_store {
                    let sessions_bg = sessions.clone();
                    let store_bg = store.clone();
                    tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_secs(30));
                        interval.tick().await; // skip the immediate first tick
                        loop {
                            interval.tick().await;
                            sessions_bg.flush(&store_bg);
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
                            let token = sync_auth.for_children();
                            match http::run_session_sync(&ctx_bin, &base_url, token.as_deref())
                                .await
                            {
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
                if let Some(ref store) = hub_store {
                    info!("Flushing session stats on shutdown");
                    sessions.flush(store);
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
                let detected = if hub_store.is_some() {
                    std::env::current_dir().ok().and_then(|cwd| {
                        match ctxone_hub::project::detect_project(&cwd, hub_store.as_ref()) {
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
                            ctxone_hub::project::DetectResult::Failed(reason) => {
                                warn!(error = %reason, "project detection failed");
                                None
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
        // Skipped in memory mode — nowhere to persist to.
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

        // (registry, shared session Arc, where to persist). None in memory mode.
        let session_persistence: Option<(
            Arc<memory_tools::SessionRegistry>,
            Arc<memory_tools::SessionStats>,
            ctxone_hub::hub_store::HubStore,
        )> = hub_store.clone().map(|store| {
            info!("Loading persisted session stats (stdio)");
            let registry = Arc::new(memory_tools::SessionRegistry::load(&store));
            // get_or_create returns the loaded Arc when the row existed (seeded
            // with prior totals), else a fresh zeroed one — either way the
            // server and registry now share this exact Arc.
            let session = registry.get_or_create(&mcp_session_id);
            (registry, session, store)
        });

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
                if let Some((ref registry, _, ref store)) = session_persistence {
                    let registry_bg = registry.clone();
                    let store_bg = store.clone();
                    tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_secs(30));
                        interval.tick().await; // skip the immediate first tick
                        loop {
                            interval.tick().await;
                            registry_bg.flush(&store_bg);
                        }
                    });
                }

                let service = oxtel_rmcp::Instrumented::new(ctx_server)
                    .with_transport("pipe")
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
                if let Some((ref registry, _, ref store)) = session_persistence {
                    info!("Flushing session stats on stdio shutdown");
                    registry.flush(store);
                }
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
    }

    info!("Hub shut down");
    Ok(())
}

/// Log what `--strict-origin` allows.
fn log_strict_origin(strict_origin: bool, allowed_origins: &[String]) {
    if !strict_origin {
        return;
    }
    if allowed_origins.is_empty() {
        warn!(
            "--strict-origin with no --allowed-origin: no browser page can call the API, \
             Lens included"
        );
    } else {
        info!(
            origins = ?allowed_origins,
            "Strict origins: browsers may call the API only from these origins"
        );
    }
}

/// Refuse a `--tenants-file` setting that can't apply, and exit EX_USAGE.
fn refuse_tenants_file(why: &str) -> ! {
    eprintln!("ctxone-hub: --tenants-file {why}");
    std::process::exit(64);
}

/// A `--pg-pool-size` (or `CTXONE_PG_POOL_SIZE`) value: a whole number of
/// connections, at least 1. Anything else exits EX_USAGE.
fn parse_pool_size(value: &str) -> usize {
    match value.trim().parse::<usize>() {
        Ok(n) if n > 0 => n,
        _ => {
            eprintln!("ctxone-hub: --pg-pool-size needs a whole number above 0, got: {value}");
            std::process::exit(64);
        }
    }
}

/// What a multi-tenant hub starts with.
struct TenantsRun {
    path: String,
    file: tenant::TenantsFile,
    /// The tenants' database, or None to keep them in memory.
    database_url: Option<String>,
    pg_pool_size: usize,
    port: u16,
    rate_limit_rpm: u32,
    agent_id: String,
    allowed_origins: Vec<String>,
    strict_origin: bool,
}

/// Serve a multi-tenant hub until Ctrl-C or SIGTERM: every request's token
/// picks its tenant, a tenant opens on its first request and closes when it's
/// idle, and the open tenants' session statistics are flushed every 30 seconds
/// and on the way out. Nothing here acts on this machine: no ASD repos, no
/// session sweep, no backups, and no flat-size refresher, whose one number
/// would be one tenant's graph size reported to all of them.
fn serve_tenants(run: TenantsRun) -> Result<(), Box<dyn std::error::Error>> {
    let (tenants, tokens) = run.file.counts();
    info!(
        path = %run.path,
        tenants,
        tokens,
        "multi-tenant mode: each request's token picks its tenant, loopback included"
    );
    info!(
        "off in multi-tenant mode, because it acts on this machine: ASD code intelligence, \
         session sync, import and autosync, help from other tools' binaries, project detection \
         reading this machine's disk, and SQLite backups"
    );
    log_strict_origin(run.strict_origin, &run.allowed_origins);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let opener: Arc<dyn tenant_cache::TenantOpener> = match &run.database_url {
                Some(url) => {
                    // Never the URL itself: it may carry a password.
                    let tls = ctxone_hub::postgres_url::TlsIntent::of(url);
                    info!(
                        storage = "postgres",
                        tls = tls.label(),
                        pool_size = run.pg_pool_size,
                        "Storage: postgres, every tenant on one pool"
                    );
                    if let Some(warning) = tls.warning() {
                        warn!("{warning}");
                    }
                    Arc::new(tenant_cache::PostgresTenants::connect(url, run.pg_pool_size).await?)
                }
                None => {
                    info!(
                        storage = "memory",
                        "Storage: in memory (ephemeral), for trying multi-tenant mode out"
                    );
                    Arc::new(tenant_cache::MemoryTenants::default())
                }
            };
            let data = tenant_cache::TenantCache::new(opener);
            let config = http::HubConfig {
                rate_limit_rpm: run.rate_limit_rpm,
                mcp_http: true,
                agent_id: run.agent_id,
                allowed_origins: run.allowed_origins,
                strict_origin: run.strict_origin,
                ..Default::default()
            };
            let app = http::multi_tenant_router(
                config,
                http::MultiTenant {
                    resolver: Arc::new(run.file),
                    data: data.clone(),
                },
            );
            let upkeep = data.spawn_upkeep(tenant_cache::UPKEEP_EVERY, tenant_cache::IDLE_AFTER);

            let addr = format!("0.0.0.0:{}", run.port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            info!(port = run.port, addr = %addr, rate_limit_rpm = run.rate_limit_rpm, "HTTP API listening");
            let result = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await;

            upkeep.abort();
            info!("Flushing open tenants' session stats on shutdown");
            let flushing = data.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || flushing.flush_all()).await {
                error!(error = %e, "flushing the open tenants failed");
            }
            if let Err(e) = result {
                error!(error = %e, "HTTP server error");
                return Err::<(), Box<dyn std::error::Error>>(e.into());
            }
            info!("Hub shut down");
            Ok(())
        })
}

/// Whether environment variable `name` is set to a true value (`1`, `true`,
/// `yes` or `on`, any case).
fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}
