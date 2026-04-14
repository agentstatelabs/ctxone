use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;
use std::path::PathBuf;

// -- Exit codes (sysexits.h-style) --
#[allow(dead_code)]
const EX_OK: i32 = 0;
#[allow(dead_code)]
const EX_USAGE: i32 = 64; // bad arguments (clap handles this)
const EX_DATAERR: i32 = 65; // bad input data
const EX_NOINPUT: i32 = 66; // input not found / not readable
const EX_UNAVAILABLE: i32 = 69; // service unavailable (hub unreachable)
const EX_SOFTWARE: i32 = 70; // internal software error
const EX_IOERR: i32 = 74; // I/O error
const EX_PROTOCOL: i32 = 76; // remote protocol error / server error

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable (default)
    Text,
    /// JSON for tool-chain piping (jq, etc.)
    Json,
    /// Minimal output: one identifier per line, nothing else
    Id,
}

#[derive(Parser)]
#[command(name = "ctx", about = "CtxOne — AI agent memory CLI", version)]
struct Cli {
    /// Hub server URL (env: CTX_SERVER)
    #[arg(
        long,
        env = "CTX_SERVER",
        default_value = "http://localhost:3001",
        global = true
    )]
    server: String,

    /// Branch / ref to read from and write to (env: CTX_BRANCH)
    #[arg(long, env = "CTX_BRANCH", default_value = "main", global = true)]
    branch: String,

    /// Output format: text (human), json (for jq), id (minimal) (env: CTX_FORMAT)
    #[arg(long, env = "CTX_FORMAT", value_enum, default_value_t = OutputFormat::Text, global = true)]
    format: OutputFormat,

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
    /// Seed the Hub with realistic demo data and show live token savings
    Demo,
    /// List pinned memories (always-included critical context)
    Pinned,
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
    /// Search the graph for a literal substring. Unlike recall, this is not
    /// LLM-oriented — it returns full matching paths and values, no budget.
    Search {
        /// Substring to search for (case-insensitive)
        query: String,
        /// Max results (default: 50)
        #[arg(short = 'n', long, default_value_t = 50)]
        max: usize,
    },
    /// List paths in the memory graph under a prefix
    Ls {
        /// Prefix to list under (default: /)
        #[arg(default_value = "/")]
        prefix: String,
        /// Max tree depth (default: 50)
        #[arg(long, default_value_t = 50)]
        max_depth: usize,
    },
    /// Read a value at a specific path
    Get {
        /// Path to read (e.g., /memory/facts/abc)
        path: String,
    },
    /// Show recent commit history
    Log {
        /// Max commits to show (default: 20)
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Show provenance chain for a path (who wrote it, when, why)
    Blame {
        /// Path to blame
        path: String,
    },
    /// Tail -f style live monitor of new commits
    Tail {
        /// Polling interval in milliseconds (default: 2000)
        #[arg(long, default_value_t = 2000)]
        interval: u64,
    },
    /// List all branches
    Branches,
    /// Create a new branch
    Branch {
        /// Name of the new branch
        name: String,
        /// Ref to branch from (default: main)
        #[arg(long, default_value = "main")]
        from: String,
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

// -- Output / error helpers --

/// Extract the "id-like" value from an object, preferring names over paths.
fn extract_id(v: &Value) -> Option<&str> {
    // Objects with a `name` field (branches, etc.) use it.
    // Otherwise fall back through commit_id → id → path.
    for key in ["name", "commit_id", "id", "path"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s);
        }
    }
    None
}

/// Render a Value according to the chosen output format.
/// For `Text`, calls the supplied closure with the parsed value; for `Json`,
/// prints pretty JSON; for `Id`, extracts sensible identifier fields.
fn emit<F: FnOnce(&Value)>(format: OutputFormat, value: &Value, text_fn: F) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_default()
            );
        }
        OutputFormat::Text => text_fn(value),
        OutputFormat::Id => {
            // Arrays of strings: print each string directly
            if let Some(arr) = value.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        println!("{}", s);
                    } else if let Some(id) = extract_id(item) {
                        println!("{}", id);
                    }
                }
                return;
            }
            // Scalar string
            if let Some(s) = value.as_str() {
                println!("{}", s);
                return;
            }
            // Object with a known id field
            if let Some(id) = extract_id(value) {
                println!("{}", id);
            }
        }
    }
}

/// Map an HTTP error response to a sysexits-style exit code and print a diagnostic.
async fn http_error_exit(resp: reqwest::Response, context: &str) -> ! {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    eprintln!("{}: {} — {}", context, status, body);
    let code = if status.is_server_error() {
        EX_PROTOCOL
    } else if status.as_u16() == 404 {
        EX_NOINPUT
    } else if status.is_client_error() {
        EX_DATAERR
    } else {
        EX_SOFTWARE
    };
    std::process::exit(code);
}

/// Handle a reqwest error (network failure) and exit as unavailable.
fn unreachable_exit(server: &str, e: reqwest::Error) -> ! {
    eprintln!("Hub unreachable ({}): {}", server, e);
    std::process::exit(EX_UNAVAILABLE);
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
            // Read fact from stdin if "-"
            let fact = if fact == "-" {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("Failed to read stdin: {}", e);
                    std::process::exit(EX_IOERR);
                }
                buf.trim().to_string()
            } else {
                fact
            };

            if fact.is_empty() {
                eprintln!("Refusing to store an empty fact");
                std::process::exit(EX_DATAERR);
            }

            let mut body = serde_json::json!({
                "fact": fact.clone(),
                "importance": importance,
                "ref": cli.branch,
            });
            if let Some(ctx) = context {
                body["context"] = serde_json::json!(ctx);
            }
            if let Some(tags) = tags {
                body["tags"] = serde_json::json!(tags);
            }

            let resp = match reqwest::Client::new()
                .post(format!("{}/api/memory/remember", cli.server))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };

            if !resp.status().is_success() {
                http_error_exit(resp, "remember failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                println!("Remembered: {}", fact);
                if let Some(path) = v.get("path").and_then(|x| x.as_str()) {
                    println!("  path: {}", path);
                }
                if let Some(id) = v.get("commit_id").and_then(|x| x.as_str()) {
                    println!("  commit: {}", id);
                }
            });
        }
        Commands::Recall { topic, budget } => {
            let url = format!(
                "{}/api/memory/recall?topic={}&budget={}&ref={}",
                cli.server,
                urlencoding(&topic),
                budget,
                urlencoding(&cli.branch),
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "recall failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let empty_vec = vec![];
                let results = v
                    .get("results")
                    .and_then(|x| x.as_array())
                    .unwrap_or(&empty_vec);
                if results.is_empty() {
                    println!("No memories found for '{}'", topic);
                    return;
                }
                let mut printed_divider = false;
                for r in results {
                    let is_pinned = r.get("pinned").and_then(|x| x.as_bool()).unwrap_or(false);
                    let path = r.get("path").and_then(|x| x.as_str()).unwrap_or("");

                    if is_pinned {
                        let title = r.get("title").and_then(|x| x.as_str()).unwrap_or("");
                        let body = r.get("body").and_then(|x| x.as_str()).unwrap_or("");
                        println!("[PINNED] {}", title);
                        for line in body.lines().take(3) {
                            println!("  {}", line);
                        }
                        if body.lines().count() > 3 {
                            println!("  ...");
                        }
                    } else {
                        if !printed_divider {
                            println!("\n--- topic matches ---");
                            printed_divider = true;
                        }
                        let value = r.get("value").and_then(|x| x.as_str()).unwrap_or("");
                        println!("{}", value);
                        println!("  ({})", path);
                    }
                }

                let pinned_count = v.get("pinned_count").and_then(|x| x.as_u64()).unwrap_or(0);
                let topic_matches = v.get("topic_matches").and_then(|x| x.as_u64()).unwrap_or(0);
                let sent = v
                    .get("ctx_tokens_sent")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let flat = v
                    .get("ctx_tokens_estimated_flat")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let ratio = v
                    .get("ctx_savings_ratio")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0);
                println!(
                    "\n{} pinned + {} topic matches, {} tokens sent (flat would be ~{}, {:.1}x savings)",
                    pinned_count, topic_matches, sent, flat, ratio
                );
            });
        }
        Commands::Context { project } => {
            let url = format!(
                "{}/api/memory/context/{}?ref={}",
                cli.server,
                urlencoding(&project),
                urlencoding(&cli.branch),
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "context failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                if let Some(ctx) = v.get("context") {
                    println!("{}", serde_json::to_string_pretty(ctx).unwrap_or_default());
                } else {
                    println!("No context found for '{}'", project);
                }
            });
        }
        Commands::Status => {
            let health_url = format!("{}/api/health", cli.server);
            let reachable = reqwest::get(&health_url)
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);

            if !reachable {
                emit(
                    cli.format,
                    &serde_json::json!({
                        "connected": false,
                        "server": cli.server,
                    }),
                    |_| println!("Hub: unreachable ({})", cli.server),
                );
                std::process::exit(EX_UNAVAILABLE);
            }

            let mut out = serde_json::json!({
                "connected": true,
                "server": cli.server,
            });
            if let Ok(r) = reqwest::get(format!("{}/api/stats/tokens", cli.server)).await
                && let Ok(parsed) = r.json::<Value>().await
            {
                out["tokens"] = parsed;
            }
            emit(cli.format, &out, |v| {
                println!("Hub: connected ({})", cli.server);
                if let Some(t) = v.get("tokens") {
                    let used = t
                        .get("session_tokens_used")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    let saved = t
                        .get("session_tokens_saved")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    let ratio = t
                        .get("cumulative_ratio")
                        .and_then(|x| x.as_f64())
                        .unwrap_or(0.0);
                    println!(
                        "Session: {} tokens used, {} saved ({:.1}x)",
                        used, saved, ratio
                    );
                }
            });
        }
        Commands::Stats => {
            let resp = match reqwest::get(format!("{}/api/stats/tokens", cli.server)).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "stats failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let used = v
                    .get("session_tokens_used")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let saved = v
                    .get("session_tokens_saved")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let graph_tokens = v
                    .get("total_graph_size_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let ratio = v
                    .get("cumulative_ratio")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0);

                println!("CtxOne Token Savings");
                println!("  graph size:   {} tokens", graph_tokens);
                println!("  tokens sent:  {}", used);
                println!("  tokens saved: {}", saved);
                println!("  savings:      {:.1}x", ratio);
            });
        }
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
        Commands::Demo => {
            run_demo(&cli.server).await?;
        }
        Commands::Pinned => {
            let resp = match reqwest::get(format!("{}/api/memory/pinned", cli.server)).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "pinned failed").await;
            }
            let items: Vec<Value> = resp.json().await?;

            // Structured group for JSON output
            use std::collections::BTreeMap;
            type Section = (Option<String>, Option<String>);
            let mut grouped: BTreeMap<String, BTreeMap<String, Section>> = BTreeMap::new();

            for item in &items {
                let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let value = item.get("value");
                let parts: Vec<&str> = path.split('/').collect();
                if parts.len() < 6 {
                    continue;
                }
                let source = parts[3].to_string();
                let slug = parts[4].to_string();
                let field = parts[5];
                let text = value.and_then(|v| v.as_str()).unwrap_or("").to_string();
                let section_entry = grouped
                    .entry(source)
                    .or_default()
                    .entry(slug)
                    .or_insert((None, None));
                match field {
                    "title" => section_entry.0 = Some(text),
                    "body" => section_entry.1 = Some(text),
                    _ => {}
                }
            }

            // Build a serializable representation for JSON mode
            let mut json_sources: Vec<Value> = Vec::new();
            for (source, sections) in &grouped {
                let mut json_sections: Vec<Value> = Vec::new();
                for (slug, (title, body)) in sections {
                    if let (Some(t), Some(b)) = (title, body) {
                        json_sections.push(serde_json::json!({
                            "slug": slug,
                            "title": t,
                            "body": b,
                        }));
                    }
                }
                json_sources.push(serde_json::json!({
                    "source": source,
                    "sections": json_sections,
                }));
            }
            let total_sections: usize = json_sources
                .iter()
                .filter_map(|s| s.get("sections").and_then(|x| x.as_array()))
                .map(|a| a.len())
                .sum();
            let out = serde_json::json!({
                "total_sections": total_sections,
                "sources": json_sources,
            });

            emit(cli.format, &out, |_| {
                if grouped.is_empty() {
                    println!("No pinned memories.");
                    println!("Add some with: ctx prime <file.md> --pin");
                    return;
                }
                for (source, sections) in &grouped {
                    println!("[{}]", source);
                    for (title, body) in sections.values() {
                        if let (Some(t), Some(b)) = (title, body) {
                            println!("  {}", t);
                            for line in b.lines().take(2) {
                                println!("    {}", line);
                            }
                            if b.lines().count() > 2 {
                                println!("    ...");
                            }
                        }
                    }
                    println!();
                }
                println!(
                    "{} pinned sections across {} sources",
                    total_sections,
                    grouped.len()
                );
            });
        }
        Commands::Prime { file, pin, source } => {
            // Read from stdin if file is "-", otherwise from the path
            let (content, display_name) = if file == "-" {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("Failed to read stdin: {}", e);
                    std::process::exit(EX_IOERR);
                }
                (buf, "<stdin>".to_string())
            } else {
                match std::fs::read_to_string(&file) {
                    Ok(c) => (c, file.clone()),
                    Err(e) => {
                        eprintln!("Cannot read {}: {}", file, e);
                        std::process::exit(EX_NOINPUT);
                    }
                }
            };

            let sections = parse_markdown_sections(&content);
            if sections.is_empty() {
                eprintln!(
                    "No sections found in {}. Add H1 or H2 headings to structure the content.",
                    display_name
                );
                std::process::exit(EX_DATAERR);
            }

            let source_name = source.unwrap_or_else(|| {
                if file == "-" {
                    "stdin".to_string()
                } else {
                    std::path::Path::new(&file)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("default")
                        .to_string()
                }
            });

            let body = serde_json::json!({
                "source": source_name,
                "pinned": pin,
                "sections": sections,
                "ref": cli.branch,
            });

            let resp = match reqwest::Client::new()
                .post(format!("{}/api/memory/prime", cli.server))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };

            if !resp.status().is_success() {
                http_error_exit(resp, "prime failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let count = v
                    .get("sections_written")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let kind = if pin { "pinned" } else { "primed" };
                println!(
                    "{} {} sections from {} under source '{}'",
                    kind, count, display_name, source_name
                );
                if pin {
                    println!("These facts will be included in every recall response.");
                }
            });
        }
        Commands::Search { query, max } => {
            let url = format!(
                "{}/api/state/{}/search?query={}&max_results={}",
                cli.server,
                urlencoding(&cli.branch),
                urlencoding(&query),
                max,
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "search failed").await;
            }
            let results: Vec<Value> = resp.json().await?;
            let value = Value::Array(results.clone());
            emit(cli.format, &value, |_| {
                if results.is_empty() {
                    println!("No matches for '{}'", query);
                } else {
                    for r in &results {
                        let path = r.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        let val = r.get("value").and_then(|v| v.as_str()).unwrap_or("");
                        println!("{}", path);
                        println!("  {}", val);
                    }
                    println!(
                        "\n{} match{}",
                        results.len(),
                        if results.len() == 1 { "" } else { "es" }
                    );
                }
            });
        }
        Commands::Ls { prefix, max_depth } => {
            let url = format!(
                "{}/api/state/{}/paths?prefix={}&max_depth={}",
                cli.server,
                urlencoding(&cli.branch),
                urlencoding(&prefix),
                max_depth,
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "ls failed").await;
            }
            let paths: Vec<String> = resp.json().await?;
            let value = serde_json::json!(paths);
            emit(cli.format, &value, |_| {
                if paths.is_empty() {
                    println!("No paths under {}", prefix);
                } else {
                    for p in &paths {
                        println!("{}", p);
                    }
                    println!("\n{} paths", paths.len());
                }
            });
        }
        Commands::Get { path } => {
            let url = format!(
                "{}/api/state/{}?path={}",
                cli.server,
                urlencoding(&cli.branch),
                urlencoding(&path),
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "get failed").await;
            }
            let value: Value = resp.json().await?;
            emit(cli.format, &value, |v| {
                println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
            });
        }
        Commands::Log { limit } => {
            let url = format!(
                "{}/api/log/{}?limit={}",
                cli.server,
                urlencoding(&cli.branch),
                limit,
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "log failed").await;
            }
            let commits: Vec<Value> = resp.json().await?;
            let value = Value::Array(commits.clone());
            emit(cli.format, &value, |_| print_commits(&commits));
        }
        Commands::Blame { path } => {
            let url = format!(
                "{}/api/blame/{}?path={}",
                cli.server,
                urlencoding(&cli.branch),
                urlencoding(&path),
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "blame failed").await;
            }
            let value: Value = resp.json().await?;
            emit(cli.format, &value, |v| {
                println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
            });
        }
        Commands::Tail { interval } => {
            run_tail(&cli.server, &cli.branch, interval).await?;
        }
        Commands::Branches => {
            let resp = match reqwest::get(format!("{}/api/branches", cli.server)).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "branches failed").await;
            }
            let branches: Vec<Value> = resp.json().await?;
            let value = Value::Array(branches.clone());
            emit(cli.format, &value, |_| {
                if branches.is_empty() {
                    println!("No branches.");
                } else {
                    for b in &branches {
                        let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let marker = if name == cli.branch { "*" } else { " " };
                        println!("{} {:30}  {}", marker, name, id);
                    }
                }
            });
        }
        Commands::Branch { name, from } => {
            let body = serde_json::json!({ "name": name, "from": from });
            let resp = match reqwest::Client::new()
                .post(format!("{}/api/branches", cli.server))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "branch create failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let commit = v.get("commit_id").and_then(|x| x.as_str()).unwrap_or("");
                println!("Branch '{}' created from '{}' at {}", name, from, commit);
            });
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

fn print_commits(commits: &[serde_json::Value]) {
    if commits.is_empty() {
        println!("No commits.");
        return;
    }
    for c in commits {
        let ts = c
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .get(..19)
            .unwrap_or("");
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let category = c
            .get("intent")
            .and_then(|i| i.get("category"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let desc = c
            .get("intent")
            .and_then(|i| i.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let conf = c.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        println!(
            "{}  {}  [{}]  {}  ({:.0}%)",
            ts,
            id,
            category,
            desc,
            conf * 100.0
        );
    }
}

async fn run_tail(
    server: &str,
    branch: &str,
    interval_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashSet;
    use std::time::Duration;

    println!(
        "Watching {} for new commits (every {}ms, Ctrl-C to stop)",
        branch, interval_ms
    );
    println!();

    let mut seen: HashSet<String> = HashSet::new();
    let mut first = true;

    loop {
        let url = format!("{}/api/log/{}?limit=20", server, urlencoding(branch),);
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(commits) = resp.json::<Vec<serde_json::Value>>().await {
                    // Print in oldest-first order so new ones scroll at the bottom
                    let mut fresh: Vec<&serde_json::Value> = commits
                        .iter()
                        .filter(|c| {
                            let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            !seen.contains(id)
                        })
                        .collect();
                    fresh.reverse();

                    if first {
                        // On first pass, show the most recent 5 as history
                        let history: Vec<serde_json::Value> =
                            commits.iter().take(5).rev().cloned().collect();
                        print_commits(&history);
                        for c in &commits {
                            if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                                seen.insert(id.to_string());
                            }
                        }
                        first = false;
                    } else {
                        for c in &fresh {
                            print_commits(std::slice::from_ref(*c));
                            if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                                seen.insert(id.to_string());
                            }
                        }
                    }
                }
            }
            Ok(resp) => {
                eprintln!("Error: {}", resp.status());
            }
            Err(e) => {
                eprintln!("Hub unreachable: {}", e);
            }
        }

        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
}

async fn run_demo(server: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Verify Hub is reachable first
    match reqwest::get(format!("{}/api/health", server)).await {
        Ok(r) if r.status().is_success() => {}
        _ => {
            eprintln!(
                "Hub unreachable at {}. Start it with: ctx serve --http",
                server
            );
            std::process::exit(1);
        }
    }

    println!("Seeding demo memory graph...\n");

    // Realistic project facts grouped by context
    let seed: &[(&str, &str, &str)] = &[
        // Licensing / legal
        (
            "licensing",
            "high",
            "CtxOne is licensed under BSL-1.1 with automatic Apache 2.0 conversion after 4 years",
        ),
        (
            "licensing",
            "high",
            "The engine (AgentStateGraph) is BSL-1.1 licensed by the same author",
        ),
        (
            "licensing",
            "medium",
            "Commercial licensing contact: info@agentstatelabs.com",
        ),
        // Architecture
        (
            "architecture",
            "high",
            "CtxOne Hub wraps AgentStateGraph with a token-tracking memory API",
        ),
        (
            "architecture",
            "high",
            "Lens is a SvelteKit web app that reads the Hub over HTTP",
        ),
        (
            "architecture",
            "high",
            "The ctx CLI is Rust with clap, talks to the Hub over HTTP",
        ),
        (
            "architecture",
            "medium",
            "Default database path is ~/.ctxone/memory.db for shared memory across tools",
        ),
        (
            "architecture",
            "medium",
            "The Hub exposes both MCP stdio mode and REST HTTP mode",
        ),
        (
            "architecture",
            "low",
            "The engine uses blake3 for content hashing and SQLite for default storage",
        ),
        // Features
        (
            "features",
            "high",
            "ctx init auto-configures Claude Code, Cursor, VS Code, Codex with MCP",
        ),
        (
            "features",
            "high",
            "ctx prime loads markdown files as pinned or searchable memories",
        ),
        (
            "features",
            "high",
            "Pinned memories are always included in every recall response",
        ),
        (
            "features",
            "medium",
            "ctx recall returns token savings metadata on every call",
        ),
        (
            "features",
            "medium",
            "ctx stats shows cumulative session token savings",
        ),
        // Token economics
        (
            "economics",
            "high",
            "Flat memory files scale O(n) on cost — every turn loads everything",
        ),
        (
            "economics",
            "high",
            "CtxOne scales O(log n) — recall loads only what's relevant",
        ),
        (
            "economics",
            "medium",
            "Typical savings: 60x tokens per session vs flat memory files",
        ),
        (
            "economics",
            "medium",
            "Enterprise ROI: mid-sized company saves ~$32k/year on token costs",
        ),
        // Team / process
        ("team", "medium", "Craig Brown is the primary maintainer"),
        (
            "team",
            "medium",
            "Pre-commit: run cargo fmt, clippy, and svelte-check",
        ),
        (
            "team",
            "low",
            "CI runs on every push to main and on tagged releases",
        ),
    ];

    let client = reqwest::Client::new();
    let mut remembered = 0;
    for (context, importance, fact) in seed {
        let body = serde_json::json!({
            "fact": fact,
            "importance": importance,
            "context": context,
        });

        let resp = client
            .post(format!("{}/api/memory/remember", server))
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            remembered += 1;
            print!(".");
            use std::io::Write;
            std::io::stdout().flush().ok();
        }
    }
    println!("\nSeeded {} facts.\n", remembered);

    // Run a few realistic recalls and show savings
    let queries = [
        ("licensing", 1500usize),
        ("architecture", 1500),
        ("tokens", 1500),
        ("Lens", 800),
    ];

    for (topic, budget) in queries {
        let resp = reqwest::get(format!(
            "{}/api/memory/recall?topic={}&budget={}",
            server,
            urlencoding(topic),
            budget
        ))
        .await?;

        if let Ok(parsed) = resp.json::<serde_json::Value>().await {
            let matches = parsed
                .get("topic_matches")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
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
                "  recall \"{}\"  →  {} matches, {} tokens sent vs {} flat ({:.1}x savings)",
                topic, matches, sent, flat, ratio
            );
        }
    }

    // Final cumulative stats
    if let Ok(resp) = reqwest::get(format!("{}/api/stats/tokens", server)).await
        && let Ok(parsed) = resp.json::<serde_json::Value>().await
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

        println!();
        println!("Cumulative savings this session:");
        println!(
            "  {} tokens sent, {} tokens saved, {:.1}x overall",
            used, saved, ratio
        );
    }

    println!();
    println!("Try: ctx recall \"your topic here\"");
    println!("Or open Lens: http://localhost:5173");

    Ok(())
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
