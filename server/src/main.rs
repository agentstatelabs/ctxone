//! CtxOne Hub — MCP + HTTP server for AI agent memory.
//!
//! Wraps AgentStateGraph with higher-level memory operations and token tracking.
//!
//! Run as MCP server (stdio):  ctxone-hub
//! Run as HTTP server:         ctxone-hub --http
//! Options:                    ctxone-hub --storage memory
//!                             ctxone-hub --path /data/ctxone.db
//!
//! Logging is controlled via the `RUST_LOG` env var (see `tracing-subscriber`
//! docs). Default level is `info`. Examples:
//!     RUST_LOG=debug ctxone-hub --http
//!     RUST_LOG=ctxone_hub=trace ctxone-hub --http
//! All logs go to stderr so they never corrupt the MCP stdio JSON stream.

use ctxone_hub::{http, memory_tools};

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
    let mut db_path = "./ctxone.db".to_string();
    let mut database_url = String::new();
    let mut tenant_id = "default".to_string();
    let mut http_mode = false;
    let mut http_port: u16 = 3001;

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
            "--port" => {
                i += 1;
                if i < args.len() {
                    http_port = args[i].parse().unwrap_or(3001);
                }
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
                eprintln!();
                eprintln!("OPTIONS:");
                eprintln!(
                    "  -s, --storage <TYPE>  Storage backend: sqlite (default), memory, or postgres"
                );
                eprintln!("  -p, --path <PATH>     SQLite database path (default: ./ctxone.db)");
                eprintln!("      --database-url <URL>  Postgres connection URL");
                eprintln!(
                    "      --tenant <ID>     Tenant ID for multi-tenant Postgres (default: \"default\")"
                );
                eprintln!("      --port <PORT>     HTTP port (default: 3001, requires --http)");
                eprintln!("  -h, --help            Print help");
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
            _ => {}
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
            info!(storage = "sqlite", path = %db_path, "Storage: sqlite");
            let storage = SqliteStorage::open(&db_path)?;
            Arc::new(Repository::new(Box::new(storage)))
        }
    };

    repo.init()?;

    if http_mode {
        let addr = format!("0.0.0.0:{}", http_port);
        info!(port = http_port, addr = %addr, "HTTP API listening");
        info!("Try: curl http://localhost:{}/api/health", http_port);

        // Session starts with a dirty flat-size; the first read will populate it lazily.
        let session = Arc::new(memory_tools::SessionStats::new());

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let app = http::router(repo.clone(), session);
                let listener = tokio::net::TcpListener::bind(&addr).await?;
                if let Err(e) = axum::serve(listener, app).await {
                    error!(error = %e, "HTTP server error");
                    return Err::<(), Box<dyn std::error::Error>>(e.into());
                }
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
    } else {
        info!(
            transport = "stdio",
            "MCP server waiting for client (tools: remember, recall, prime, context, \
             summarize_session, what_changed_since, why_did_we)"
        );

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let service = memory_tools::CtxOneServer::new(repo)
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
