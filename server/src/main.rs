//! CtxOne Hub — MCP + HTTP server for AI agent memory.
//!
//! Wraps AgentStateGraph with higher-level memory operations and token tracking.
//!
//! Run as MCP server (stdio):  ctxone-hub
//! Run as HTTP server:         ctxone-hub --http
//! Options:                    ctxone-hub --storage memory
//!                             ctxone-hub --path /data/ctxone.db

use ctxone_hub::{http, memory_tools};

use std::sync::Arc;

use agentstategraph::Repository;
use agentstategraph_storage::{MemoryStorage, SqliteStorage};
use rmcp::ServiceExt;

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

    eprintln!("CtxOne Hub v{}", env!("CARGO_PKG_VERSION"));

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
            eprintln!("Storage: in-memory (ephemeral)");
            Arc::new(Repository::new(Box::new(MemoryStorage::new())))
        }
        "postgres" => {
            if database_url.is_empty() {
                eprintln!("Error: --database-url or DATABASE_URL required for postgres storage");
                std::process::exit(1);
            }
            eprintln!("Storage: postgres (tenant: {})", tenant_id);
            let rt = tokio::runtime::Runtime::new()?;
            let storage = rt.block_on(async {
                agentstategraph_storage::PostgresStorage::connect_tenant(&database_url, &tenant_id)
                    .await
            })?;
            Arc::new(Repository::new(Box::new(storage)))
        }
        _ => {
            eprintln!("Storage: {}", db_path);
            let storage = SqliteStorage::open(&db_path)?;
            Arc::new(Repository::new(Box::new(storage)))
        }
    };

    repo.init()?;

    if http_mode {
        eprintln!("HTTP API listening on http://0.0.0.0:{}", http_port);
        eprintln!("Try: curl http://localhost:{}/api/health", http_port);

        // Session starts with a dirty flat-size; the first read will populate it lazily.
        let session = Arc::new(memory_tools::SessionStats::new());

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let app = http::router(repo.clone(), session);
                let addr = format!("0.0.0.0:{}", http_port);
                let listener = tokio::net::TcpListener::bind(&addr).await?;
                axum::serve(listener, app).await?;
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
    } else {
        eprintln!("MCP server waiting for client on stdio...");
        eprintln!(
            "Tools: remember, recall, context, summarize_session, what_changed_since, why_did_we"
        );

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let service = memory_tools::CtxOneServer::new(repo)
                    .serve(rmcp::transport::stdio())
                    .await
                    .map_err(|e| format!("MCP server error: {}", e))?;

                service.waiting().await?;
                Ok::<(), Box<dyn std::error::Error>>(())
            })?;
    }

    eprintln!("Hub shut down.");
    Ok(())
}
