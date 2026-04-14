use clap::{Parser, Subcommand};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ctx", about = "CtxOne — AI agent memory CLI", version)]
struct Cli {
    /// Hub server URL
    #[arg(long, default_value = "http://localhost:3001", global = true)]
    server: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Store a fact in agent memory
    Remember {
        /// The fact to remember
        fact: String,
        /// Importance: high, medium, low
        #[arg(short, long, default_value = "medium")]
        importance: String,
        /// Context/category
        #[arg(short, long)]
        context: Option<String>,
        /// Tags for queryability
        #[arg(short, long)]
        tags: Option<Vec<String>>,
    },
    /// Retrieve relevant memories for a topic
    Recall {
        /// Topic to recall
        topic: String,
        /// Token budget
        #[arg(short, long, default_value_t = 1500)]
        budget: usize,
    },
    /// Load full context for a project
    Context {
        /// Project name
        project: String,
    },
    /// Show Hub status and connection info
    Status,
    /// Show token savings statistics
    Stats,
    /// Start the CtxOne Hub server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value_t = 3001)]
        port: u16,
        /// Storage backend: sqlite, postgres, memory
        #[arg(long, default_value = "sqlite")]
        storage: String,
        /// Database path (for sqlite)
        #[arg(long, default_value = "./ctxone.db")]
        path: String,
        /// Also start HTTP API server
        #[arg(long)]
        http: bool,
    },
    /// Auto-detect and configure AI tools with CtxOne MCP server
    Init {
        /// Install globally (user-level config) vs project-only
        #[arg(long)]
        global: bool,
        /// Install for current project only (default)
        #[arg(long)]
        project: bool,
        /// Target a specific tool: claude, cursor, vscode, codex, gemini
        #[arg(long)]
        tool: Option<String>,
        /// Show what would be written without writing
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Remember {
            fact,
            importance,
            context,
            tags,
        } => {
            let mut body = serde_json::json!({
                "path": format!("/memory/facts/{}", uuid_v4()),
                "value": fact,
                "intent_category": "Observe",
                "intent_description": format!("Remember: {}", &fact[..fact.len().min(60)]),
                "confidence": importance_to_confidence(&importance),
            });
            if let Some(tags) = tags {
                body["tags"] = serde_json::json!(tags);
            }
            if let Some(ctx) = &context {
                body["path"] = serde_json::json!(format!("/memory/{}/{}", ctx, uuid_v4()));
            }

            let resp = reqwest::Client::new()
                .post(format!("{}/api/state/main/set", cli.server))
                .json(&body)
                .send()
                .await?;

            if resp.status().is_success() {
                let text = resp.text().await?;
                println!("Remembered: {}", fact);
                println!("{}", text);
            } else {
                eprintln!("Error: {}", resp.status());
            }
        }
        Commands::Recall { topic, budget: _ } => {
            let resp = reqwest::get(format!(
                "{}/api/state/main/search?query={}&max_results=20",
                cli.server,
                urlencoding(&topic)
            ))
            .await?;

            if resp.status().is_success() {
                let text = resp.text().await?;
                println!("{}", text);
            } else {
                eprintln!("Error: {}", resp.status());
            }
        }
        Commands::Context { project } => {
            let resp = reqwest::get(format!(
                "{}/api/state/main?path=/memory/projects/{}",
                cli.server, project
            ))
            .await?;

            if resp.status().is_success() {
                let text = resp.text().await?;
                println!("{}", text);
            } else {
                eprintln!("Error: {}", resp.status());
            }
        }
        Commands::Status => {
            print!("Hub: ");
            match reqwest::get(format!("{}/api/health", cli.server)).await {
                Ok(resp) if resp.status().is_success() => {
                    println!("connected ({})", cli.server);
                    // Also fetch stats
                    if let Ok(stats) = reqwest::get(format!("{}/api/stats/main", cli.server)).await
                    {
                        if stats.status().is_success() {
                            if let Ok(text) = stats.text().await {
                                println!("\n{}", text);
                            }
                        }
                    }
                }
                Ok(resp) => println!("error {} ({})", resp.status(), cli.server),
                Err(_) => println!("unreachable ({})", cli.server),
            }
        }
        Commands::Stats => {
            match reqwest::get(format!("{}/api/stats/tokens", cli.server)).await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(text) = resp.text().await {
                        println!("{}", text);
                    }
                }
                Ok(_) => println!("Token stats not available yet."),
                Err(_) => eprintln!("Hub unreachable ({})", cli.server),
            }
        }
        Commands::Serve {
            port,
            storage,
            path,
            http,
        } => {
            // Launch ctxone-hub binary
            let hub_bin = find_hub_binary();
            let mut args = vec![];
            if http {
                args.push("--http".to_string());
            }
            args.extend(["--port".to_string(), port.to_string()]);
            args.extend(["--storage".to_string(), storage]);
            args.extend(["--path".to_string(), path]);

            println!("Starting CtxOne Hub on port {}...", port);
            let status = std::process::Command::new(&hub_bin)
                .args(&args)
                .status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        Commands::Init {
            global,
            project: _,
            tool,
            dry_run,
        } => {
            init_mcp(global, tool, dry_run)?;
        }
    }

    Ok(())
}

fn importance_to_confidence(importance: &str) -> f64 {
    match importance {
        "high" => 0.95,
        "medium" => 0.7,
        "low" => 0.4,
        _ => 0.7,
    }
}

fn uuid_v4() -> String {
    // Simple timestamp-based ID for now
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    format!("{:x}", now.as_nanos())
}

fn urlencoding(s: &str) -> String {
    s.replace(' ', "%20")
        .replace('&', "%26")
        .replace('?', "%3F")
}

fn find_hub_binary() -> String {
    // Check common locations
    let candidates = [
        // Same directory as ctx
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("ctxone-hub")))
            .unwrap_or_default(),
        PathBuf::from("/usr/local/bin/ctxone-hub"),
        PathBuf::from(format!(
            "{}/.local/bin/ctxone-hub",
            std::env::var("HOME").unwrap_or_default()
        )),
        // Also check for the engine binary directly
        PathBuf::from("/usr/local/bin/agentstategraph-mcp"),
        PathBuf::from(format!(
            "{}/.local/bin/agentstategraph-mcp",
            std::env::var("HOME").unwrap_or_default()
        )),
    ];

    for c in &candidates {
        if c.exists() {
            return c.to_string_lossy().to_string();
        }
    }

    // Fall back to PATH lookup
    "ctxone-hub".to_string()
}

// -- MCP Init --

struct AiTool {
    name: &'static str,
    detected: bool,
    config_path: PathBuf,
    config_type: ConfigType,
}

enum ConfigType {
    /// JSON with mcpServers key
    McpJson,
    /// TOML config
    Toml,
}

fn detect_tools(global: bool) -> Vec<AiTool> {
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();

    let mut tools = vec![];

    // Claude Code — project-level .mcp.json
    let claude_path = if global {
        PathBuf::from(format!("{}/.claude/settings.json", home))
    } else {
        cwd.join(".mcp.json")
    };
    tools.push(AiTool {
        name: "Claude Code",
        detected: true, // Always available
        config_path: claude_path,
        config_type: ConfigType::McpJson,
    });

    // Claude Desktop
    let claude_desktop = PathBuf::from(format!(
        "{}/Library/Application Support/Claude/claude_desktop_config.json",
        home
    ));
    tools.push(AiTool {
        name: "Claude Desktop",
        detected: claude_desktop.exists(),
        config_path: claude_desktop,
        config_type: ConfigType::McpJson,
    });

    // Cursor
    let cursor_path = if global {
        PathBuf::from(format!("{}/.cursor/mcp.json", home))
    } else {
        cwd.join(".cursor/mcp.json")
    };
    let cursor_detected = PathBuf::from(format!("{}/.cursor", home)).exists();
    tools.push(AiTool {
        name: "Cursor",
        detected: cursor_detected,
        config_path: cursor_path,
        config_type: ConfigType::McpJson,
    });

    // VS Code
    let vscode_path = if global {
        PathBuf::from(format!(
            "{}/Library/Application Support/Code/User/settings.json",
            home
        ))
    } else {
        cwd.join(".vscode/mcp.json")
    };
    let vscode_detected = PathBuf::from(format!(
        "{}/Library/Application Support/Code",
        home
    ))
    .exists();
    tools.push(AiTool {
        name: "VS Code",
        detected: vscode_detected,
        config_path: vscode_path,
        config_type: ConfigType::McpJson,
    });

    // Codex
    let codex_path = PathBuf::from(format!("{}/.codex", home));
    tools.push(AiTool {
        name: "Codex",
        detected: codex_path.exists(),
        config_path: codex_path.join("config.toml"),
        config_type: ConfigType::Toml,
    });

    tools
}

fn mcp_server_entry() -> Value {
    let hub_bin = find_hub_binary();
    serde_json::json!({
        "command": hub_bin,
        "args": []
    })
}

fn init_mcp(global: bool, tool_filter: Option<String>, dry_run: bool) -> Result<(), Box<dyn std::error::Error>> {
    let tools = detect_tools(global);

    println!("Detected AI tools:");
    for t in &tools {
        let icon = if t.detected { "\u{2713}" } else { "\u{2717}" };
        println!("  {} {}", icon, t.name);
    }
    println!();

    let targets: Vec<&AiTool> = tools
        .iter()
        .filter(|t| t.detected)
        .filter(|t| {
            tool_filter
                .as_ref()
                .map_or(true, |f| t.name.to_lowercase().contains(&f.to_lowercase()))
        })
        .collect();

    if targets.is_empty() {
        println!("No matching AI tools detected.");
        return Ok(());
    }

    let entry = mcp_server_entry();

    for t in &targets {
        match t.config_type {
            ConfigType::McpJson => {
                let mut config: Value = if t.config_path.exists() {
                    let content = std::fs::read_to_string(&t.config_path)?;
                    serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
                } else {
                    serde_json::json!({})
                };

                config
                    .as_object_mut()
                    .unwrap()
                    .entry("mcpServers")
                    .or_insert(serde_json::json!({}))
                    .as_object_mut()
                    .unwrap()
                    .insert("ctxone".to_string(), entry.clone());

                let pretty = serde_json::to_string_pretty(&config)?;

                if dry_run {
                    println!("  [dry-run] {}: would write {}", t.name, t.config_path.display());
                    println!("  {}", pretty);
                } else {
                    if let Some(parent) = t.config_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&t.config_path, pretty)?;
                    println!("  \u{2192} {}: wrote {} \u{2713}", t.name, t.config_path.display());
                }
            }
            ConfigType::Toml => {
                if dry_run {
                    println!(
                        "  [dry-run] {}: would configure MCP in {}",
                        t.name,
                        t.config_path.display()
                    );
                } else {
                    println!(
                        "  \u{2192} {}: TOML config not yet supported, configure manually",
                        t.name
                    );
                }
            }
        }
    }

    println!();
    println!("CtxOne is ready. Try: \"remember that we use BSL-1.1 licensing\"");

    Ok(())
}
