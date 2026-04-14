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
        /// Database path (for sqlite). Defaults to ~/.ctxone/memory.db
        #[arg(long)]
        path: Option<String>,
        /// Also start HTTP API server
        #[arg(long)]
        http: bool,
    },
    /// Load a markdown file as primed memory. Use --pin to make it always-included.
    Prime {
        /// Path to the markdown file
        file: String,
        /// Pin: these memories are always included in recall (critical context)
        #[arg(long)]
        pin: bool,
        /// Source name (defaults to the file stem)
        #[arg(long)]
        source: Option<String>,
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
                "fact": fact,
                "importance": importance,
            });
            if let Some(ctx) = context {
                body["context"] = serde_json::json!(ctx);
            }
            if let Some(tags) = tags {
                body["tags"] = serde_json::json!(tags);
            }

            let resp = reqwest::Client::new()
                .post(format!("{}/api/memory/remember", cli.server))
                .json(&body)
                .send()
                .await?;

            if resp.status().is_success() {
                let parsed: serde_json::Value = resp.json().await?;
                println!("Remembered: {}", fact);
                if let Some(path) = parsed.get("path").and_then(|v| v.as_str()) {
                    println!("  path: {}", path);
                }
                if let Some(id) = parsed.get("commit_id").and_then(|v| v.as_str()) {
                    println!("  commit: {}", id);
                }
            } else {
                eprintln!("Error: {} — {}", resp.status(), resp.text().await?);
            }
        }
        Commands::Recall { topic, budget } => {
            let resp = reqwest::get(format!(
                "{}/api/memory/recall?topic={}&budget={}",
                cli.server,
                urlencoding(&topic),
                budget,
            ))
            .await?;

            if resp.status().is_success() {
                let parsed: serde_json::Value = resp.json().await?;
                let empty_vec = vec![];
                let results = parsed
                    .get("results")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&empty_vec);
                if results.is_empty() {
                    println!("No memories found for '{}'", topic);
                } else {
                    for r in results {
                        let path = r.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        let value = r.get("value").and_then(|v| v.as_str()).unwrap_or("");
                        println!("{}", value);
                        println!("  ({})", path);
                    }
                    let sent = parsed
                        .get("ctx_tokens_sent")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let flat = parsed
                        .get("ctx_tokens_estimated_flat")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let ratio = parsed
                        .get("ctx_savings_ratio")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    println!(
                        "\n{} tokens sent (flat would be ~{}, {:.1}x savings)",
                        sent, flat, ratio
                    );
                }
            } else {
                eprintln!("Error: {} — {}", resp.status(), resp.text().await?);
            }
        }
        Commands::Context { project } => {
            let resp = reqwest::get(format!(
                "{}/api/memory/context/{}",
                cli.server,
                urlencoding(&project)
            ))
            .await?;

            if resp.status().is_success() {
                let parsed: serde_json::Value = resp.json().await?;
                if let Some(ctx) = parsed.get("context") {
                    println!("{}", serde_json::to_string_pretty(ctx).unwrap_or_default());
                } else {
                    println!("No context found for '{}'", project);
                }
            } else {
                eprintln!("Error: {} — {}", resp.status(), resp.text().await?);
            }
        }
        Commands::Status => {
            print!("Hub: ");
            match reqwest::get(format!("{}/api/health", cli.server)).await {
                Ok(resp) if resp.status().is_success() => {
                    println!("connected ({})", cli.server);

                    if let Ok(r) = reqwest::get(format!("{}/api/stats/tokens", cli.server)).await
                        && let Ok(parsed) = r.json::<serde_json::Value>().await
                    {
                        let used = parsed
                            .get("session_tokens_used")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let saved = parsed
                            .get("session_tokens_saved")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let ratio = parsed
                            .get("cumulative_ratio")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        println!(
                            "Session: {} tokens used, {} saved ({:.1}x)",
                            used, saved, ratio
                        );
                    }
                }
                Ok(resp) => println!("error {} ({})", resp.status(), cli.server),
                Err(_) => println!("unreachable ({})", cli.server),
            }
        }
        Commands::Stats => match reqwest::get(format!("{}/api/stats/tokens", cli.server)).await {
            Ok(resp) if resp.status().is_success() => {
                let parsed: serde_json::Value = resp.json().await?;
                let used = parsed
                    .get("session_tokens_used")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let saved = parsed
                    .get("session_tokens_saved")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let graph_tokens = parsed
                    .get("total_graph_size_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let ratio = parsed
                    .get("cumulative_ratio")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                println!("CtxOne Token Savings");
                println!("  graph size:   {} tokens", graph_tokens);
                println!("  tokens sent:  {}", used);
                println!("  tokens saved: {}", saved);
                println!("  savings:      {:.1}x", ratio);
            }
            Ok(_) => println!("Token stats not available."),
            Err(_) => eprintln!("Hub unreachable ({})", cli.server),
        },
        Commands::Serve {
            port,
            storage,
            path,
            http,
        } => {
            let db_path = path.unwrap_or_else(canonical_db_path);
            if let Some(parent) = std::path::Path::new(&db_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }

            let hub_bin = find_hub_binary();
            let mut args = vec![];
            if http {
                args.push("--http".to_string());
            }
            args.extend(["--port".to_string(), port.to_string()]);
            args.extend(["--storage".to_string(), storage]);
            args.extend(["--path".to_string(), db_path.clone()]);

            println!("Starting CtxOne Hub on port {} (db: {})", port, db_path);
            let status = std::process::Command::new(&hub_bin).args(&args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        Commands::Prime { file, pin, source } => {
            let content = std::fs::read_to_string(&file)?;
            let sections = parse_markdown_sections(&content);

            if sections.is_empty() {
                eprintln!(
                    "No sections found in {}. Add H1 or H2 headings to structure the content.",
                    file
                );
                std::process::exit(1);
            }

            let source_name = source.unwrap_or_else(|| {
                std::path::Path::new(&file)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("default")
                    .to_string()
            });

            let body = serde_json::json!({
                "source": source_name,
                "pinned": pin,
                "sections": sections,
            });

            let resp = reqwest::Client::new()
                .post(format!("{}/api/memory/prime", cli.server))
                .json(&body)
                .send()
                .await?;

            if resp.status().is_success() {
                let parsed: serde_json::Value = resp.json().await?;
                let count = parsed
                    .get("sections_written")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let kind = if pin { "pinned" } else { "primed" };
                println!(
                    "{} {} sections from {} under source '{}'",
                    kind, count, file, source_name
                );
                if pin {
                    println!("These facts will be included in every recall response.");
                }
            } else {
                eprintln!("Error: {} — {}", resp.status(), resp.text().await?);
                std::process::exit(1);
            }
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
    let vscode_detected =
        PathBuf::from(format!("{}/Library/Application Support/Code", home)).exists();
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

#[derive(serde::Serialize)]
struct Section {
    title: String,
    body: String,
}

/// Parse a markdown document into sections split at H1 or H2 headings.
/// Content before the first heading is captured under a synthetic "Intro" section if non-empty.
fn parse_markdown_sections(content: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_body = String::new();

    for line in content.lines() {
        let trimmed = line.trim_start();
        let is_h1 = trimmed.starts_with("# ") && !trimmed.starts_with("## ");
        let is_h2 = trimmed.starts_with("## ") && !trimmed.starts_with("### ");

        if is_h1 || is_h2 {
            // Flush the current section
            if let Some(title) = current_title.take() {
                let body = current_body.trim().to_string();
                if !body.is_empty() {
                    sections.push(Section { title, body });
                }
            } else if !current_body.trim().is_empty() {
                sections.push(Section {
                    title: "Intro".to_string(),
                    body: current_body.trim().to_string(),
                });
            }
            current_body.clear();

            let prefix_len = if is_h1 { 2 } else { 3 };
            current_title = Some(trimmed[prefix_len..].trim().to_string());
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    // Flush the final section
    if let Some(title) = current_title {
        let body = current_body.trim().to_string();
        if !body.is_empty() {
            sections.push(Section { title, body });
        }
    } else if !current_body.trim().is_empty() {
        sections.push(Section {
            title: "Intro".to_string(),
            body: current_body.trim().to_string(),
        });
    }

    sections
}

fn canonical_db_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{}/.ctxone/memory.db", home)
}

fn mcp_server_entry() -> Value {
    let hub_bin = find_hub_binary();
    let db_path = canonical_db_path();

    // Ensure the parent directory exists so the Hub can create the db on first run.
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    serde_json::json!({
        "command": hub_bin,
        "args": ["--path", db_path]
    })
}

fn init_mcp(
    global: bool,
    tool_filter: Option<String>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
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
                .is_none_or(|f| t.name.to_lowercase().contains(&f.to_lowercase()))
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
                    println!(
                        "  [dry-run] {}: would write {}",
                        t.name,
                        t.config_path.display()
                    );
                    println!("  {}", pretty);
                } else {
                    if let Some(parent) = t.config_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&t.config_path, pretty)?;
                    println!(
                        "  \u{2192} {}: wrote {} \u{2713}",
                        t.name,
                        t.config_path.display()
                    );
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
