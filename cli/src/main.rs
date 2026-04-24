use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use serde_json::Value;
use std::path::PathBuf;

// -- Exit codes (sysexits.h-style) --
#[allow(dead_code)]
/// The canonical AGENTS.md shipped with this build of `ctx`. Embedded
/// at compile time so there's no "missing file" failure mode and no
/// filesystem lookup on startup. The `ctx agents install` subcommand
/// reads this (or a user-supplied override via `--file`), writes it to
/// ~/.config/ctxone/AGENTS.md if no local copy exists, and then primes
/// it into the Hub as pinned memory. See `docs/AGENTS.md` for the
/// source of truth.
const EMBEDDED_AGENTS_MD: &str = include_str!("../../docs/AGENTS.md");

/// Source name used when priming AGENTS.md into the Hub. Sections of
/// the file end up under `/memory/pinned/ctxone-agents/<slug>/`.
const AGENTS_SOURCE: &str = "ctxone-agents";

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

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
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
struct RawCli {
    /// Hub server URL (env: CTX_SERVER, config: server)
    #[arg(long, env = "CTX_SERVER", global = true)]
    server: Option<String>,

    /// Branch / ref to read and write (env: CTX_BRANCH, config: branch)
    #[arg(long, env = "CTX_BRANCH", global = true)]
    branch: Option<String>,

    /// Output format: text / json / id (env: CTX_FORMAT, config: format)
    #[arg(long, env = "CTX_FORMAT", value_enum, global = true)]
    format: Option<OutputFormat>,

    /// Session identifier sent as X-CTXone-Session header (env: CTX_SESSION)
    #[arg(long, env = "CTX_SESSION", global = true)]
    session: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

/// Fully-resolved CLI with defaults applied.
/// Priority: flag → env var → config file → hardcoded default.
struct Cli {
    server: String,
    branch: String,
    format: OutputFormat,
    session: Option<String>,
    command: Commands,
}

impl Cli {
    fn from_raw(raw: RawCli, config: &CtxConfig) -> Self {
        Self {
            server: raw
                .server
                .or_else(|| config.server.clone())
                .unwrap_or_else(|| "http://localhost:3001".to_string()),
            branch: raw
                .branch
                .or_else(|| config.branch.clone())
                .unwrap_or_else(|| "main".to_string()),
            format: raw.format.or(config.format).unwrap_or(OutputFormat::Text),
            session: raw.session,
            command: raw.command,
        }
    }

    /// Build a reqwest client with X-CTXone-Session baked in as a default header.
    fn http_client(&self) -> reqwest::Client {
        let mut builder = reqwest::Client::builder();
        if let Some(ref sid) = self.session {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Ok(val) = reqwest::header::HeaderValue::from_str(sid) {
                headers.insert("X-CTXone-Session", val);
            }
            builder = builder.default_headers(headers);
        }
        builder.build().unwrap_or_default()
    }
}

// -- Persistent config (~/.ctxone/config.toml) --

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
struct CtxConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<OutputFormat>,
}

impl CtxConfig {
    /// Path to the config file. Separate from the db path so devs can wipe
    /// the graph without losing their preferred server URL.
    ///
    /// Unix: `~/.ctxone/config.toml`
    /// Windows: `%APPDATA%\ctxone\config.toml`
    fn path() -> PathBuf {
        if cfg!(target_os = "windows") {
            dirs::config_dir()
                .map(|d| d.join("ctxone").join("config.toml"))
                .unwrap_or_else(|| PathBuf::from("./ctxone-config.toml"))
        } else {
            dirs::home_dir()
                .map(|h| h.join(".ctxone").join("config.toml"))
                .unwrap_or_else(|| PathBuf::from("./ctxone-config.toml"))
        }
    }

    fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        toml::from_str(&content).unwrap_or_default()
    }

    fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(&path, toml_str)?;
        Ok(())
    }

    fn set_key(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "server" => self.server = Some(value.to_string()),
            "branch" => self.branch = Some(value.to_string()),
            "format" => {
                let f = match value.to_lowercase().as_str() {
                    "text" => OutputFormat::Text,
                    "json" => OutputFormat::Json,
                    "id" => OutputFormat::Id,
                    _ => return Err(format!("unknown format: {} (expected text|json|id)", value)),
                };
                self.format = Some(f);
            }
            _ => {
                return Err(format!(
                    "unknown key: {} (expected server|branch|format)",
                    key
                ));
            }
        }
        Ok(())
    }

    fn get_key(&self, key: &str) -> Result<String, String> {
        match key {
            "server" => Ok(self.server.clone().unwrap_or_default()),
            "branch" => Ok(self.branch.clone().unwrap_or_default()),
            "format" => Ok(self
                .format
                .map(|f| match f {
                    OutputFormat::Text => "text",
                    OutputFormat::Json => "json",
                    OutputFormat::Id => "id",
                })
                .unwrap_or("")
                .to_string()),
            _ => Err(format!(
                "unknown key: {} (expected server|branch|format)",
                key
            )),
        }
    }

    fn unset_key(&mut self, key: &str) -> Result<(), String> {
        match key {
            "server" => self.server = None,
            "branch" => self.branch = None,
            "format" => self.format = None,
            _ => {
                return Err(format!(
                    "unknown key: {} (expected server|branch|format)",
                    key
                ));
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the current config file contents
    Show,
    /// Print the path to the config file
    Path,
    /// Get a single config value (server, branch, format)
    Get {
        /// Key: server, branch, or format
        key: String,
    },
    /// Set a config value and save it to the file
    Set {
        /// Key: server, branch, or format
        key: String,
        /// New value
        value: String,
    },
    /// Remove a key from the config file
    Unset {
        /// Key: server, branch, or format
        key: String,
    },
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
        /// Re-tokenize the response locally with tiktoken (cl100k_base) and
        /// show exact token counts next to the fast 4-char estimate.
        /// Also re-tokenizes the full graph for an exact flat baseline.
        #[arg(long)]
        exact: bool,
    },
    /// Count the exact tokens in a piece of text (tiktoken cl100k_base).
    /// Reads from stdin if no argument is given.
    Tokens {
        /// Text to tokenize. Use "-" or omit to read from stdin.
        #[arg(default_value = "-")]
        text: String,
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
        /// Serve Lens web UI at / (requires --http)
        #[arg(long)]
        lens: bool,
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
    /// Generate shell completion script (bash, zsh, fish, etc.)
    Completion {
        /// Shell to generate completions for
        shell: Shell,
    },
    /// Run end-to-end health checks and suggest fixes
    Doctor,
    /// Diff two refs (branches, tags, or commits)
    Diff {
        /// First ref (usually older / base)
        ref_a: String,
        /// Second ref (usually newer / target)
        ref_b: String,
    },
    /// Merge a source branch into a target branch
    Merge {
        /// Source branch (the one with new changes)
        source: String,
        /// Target branch to merge into (default: main)
        #[arg(long, default_value = "main")]
        into: String,
        /// Commit message describing the merge
        #[arg(short = 'm', long)]
        message: Option<String>,
    },
    /// Forget (delete) a memory at a specific path
    Forget {
        /// Path to forget (get it from ctx search or ctx ls)
        path: String,
        /// Reason, shows up in blame (default: "forgotten by user")
        #[arg(long)]
        reason: Option<String>,
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
    /// Read or write persistent defaults in ~/.ctxone/config.toml
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Auto-detect and configure AI tools with CTXone MCP server
    Init {
        /// Install globally (user-level config) vs project-only
        #[arg(long)]
        global: bool,
        /// Install for current project only (default)
        #[arg(long)]
        project: bool,
        /// Target a specific tool: claude, cursor, vscode, codex, gemini, grok
        #[arg(long)]
        tool: Option<String>,
        /// Write to an arbitrary MCP config file (JSON, with an mcpServers object).
        /// Used for MCP clients not yet directly supported by ctx init.
        /// Example: ctx init --config-path ~/.myeditor/mcp.json
        #[arg(long)]
        config_path: Option<String>,
        /// Show what would be written without writing
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive prompt to install AGENTS.md guidance.
        /// Useful in scripts that want only the MCP config step.
        #[arg(long)]
        no_agents: bool,
    },
    /// Manage the AGENTS.md guidance file — a short, pinned document
    /// that teaches AI tools how to use CTXone effectively. See
    /// `ctx agents show` for the full text. Nothing is installed
    /// automatically; `install` always prompts unless `--yes` is passed.
    Agents {
        #[command(subcommand)]
        action: AgentsAction,
    },
    /// Manage plans — a container of tasks that survives across sessions.
    /// Plans are the CTXone cure for "plan rot": unstructured markdown
    /// todos that drift from reality the moment work starts.
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },
}

/// Proof specifier — `kind:value[:note]`. Kind is one of commit|file|test|text.
fn parse_proof_spec(raw: &str) -> Result<(String, String, Option<String>), String> {
    let parts: Vec<&str> = raw.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(format!(
            "expected <kind>:<value>[:<note>] (e.g. 'commit:abc123'), got '{}'",
            raw
        ));
    }
    let kind = parts[0].to_string();
    let value = parts[1].to_string();
    let note = parts.get(2).map(|s| s.to_string());
    match kind.as_str() {
        "commit" | "file" | "test" | "text" => Ok((kind, value, note)),
        _ => Err(format!(
            "unknown proof kind '{}' (expected commit|file|test|text)",
            kind
        )),
    }
}

#[derive(Subcommand)]
enum PlanAction {
    /// Create a new plan
    New {
        /// Plan name (kebab-case, no spaces)
        name: String,
        /// Optional description
        #[arg(long, short)]
        description: Option<String>,
    },
    /// Add a task to a plan
    Add {
        /// Plan name
        plan_id: String,
        /// Task title (imperative sentence)
        title: String,
        /// Longer-form description (appended to title)
        #[arg(long, short)]
        description: Option<String>,
        /// Priority: low|medium|high|critical
        #[arg(long, default_value = "medium")]
        priority: String,
        /// Make this a subtask of another task
        #[arg(long)]
        parent: Option<String>,
        /// Agent id this task is intended for (e.g. 'claude-code',
        /// 'codex', a user email). Enables multi-agent orchestration
        /// via `ctx plan next --me`.
        #[arg(long = "assigned-to")]
        assigned_to: Option<String>,
        /// Task that must be `done` before this one can start. May be
        /// passed multiple times.
        #[arg(long = "blocks", short = 'b')]
        blocks: Vec<String>,
    },
    /// Mark a task in-progress
    Start {
        plan_id: String,
        task_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Mark a task done (requires proof)
    Done {
        plan_id: String,
        task_id: String,
        /// Proof spec: kind:value[:note]. Kind is commit|file|test|text.
        #[arg(long, short)]
        proof: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Mark a task abandoned (requires reason)
    Abandon {
        plan_id: String,
        task_id: String,
        /// Why this task is being abandoned. Shows up in blame.
        #[arg(long, short)]
        reason: String,
    },
    /// Show the next pickable task
    Next {
        plan_id: String,
        /// Filter to tasks assigned to this agent id. Mutually exclusive with --me.
        #[arg(long = "assigned-to")]
        assigned_to: Option<String>,
        /// Shortcut for --assigned-to <session-agent>. Uses X-CTXone-Agent
        /// (CTX_AGENT_ID env or the config default).
        #[arg(long)]
        me: bool,
        /// Include unassigned tasks alongside assigned ones (default true)
        #[arg(long = "include-unassigned", default_value_t = true)]
        include_unassigned: bool,
        /// Restrict strictly to tasks explicitly assigned
        #[arg(long = "assigned-only")]
        assigned_only: bool,
    },
    /// List plans
    List {
        /// Filter by status: active|completed|archived
        #[arg(long)]
        status: Option<String>,
    },
    /// Show a plan with its tasks
    Show { plan_id: String },
    /// Archive a plan (soft — task data preserved)
    Archive { plan_id: String },
}

/// Subcommands under `ctx agents`. Everything here operates on the
/// user-editable `~/.config/ctxone/AGENTS.md` file and the pinned
/// memory path `/memory/pinned/ctxone-agents/*`.
#[derive(Subcommand)]
enum AgentsAction {
    /// Write AGENTS.md to disk (if not present) and prime it as
    /// pinned memory in the Hub. Shows the full content first and
    /// asks for confirmation, unless `--yes` is passed.
    Install {
        /// Use a custom AGENTS.md file instead of the embedded default.
        /// Useful when you've already edited your copy and want to
        /// re-prime after changes.
        #[arg(long)]
        file: Option<String>,
        /// Skip the confirmation prompt. Required for non-interactive
        /// scripts.
        #[arg(long)]
        yes: bool,
        /// Show the file content and exit without priming.
        #[arg(long)]
        show: bool,
    },
    /// Print the current AGENTS.md content. If you've edited your
    /// local copy at ~/.config/ctxone/AGENTS.md, prints that;
    /// otherwise prints the embedded default.
    Show,
    /// Report whether AGENTS.md is primed in the Hub. Queries
    /// /memory/pinned/ctxone-agents and reports section count + where
    /// the disk copy lives.
    Status,
    /// Remove the primed AGENTS.md from the Hub's memory graph. Does
    /// NOT delete the local file — use `rm ~/.config/ctxone/AGENTS.md`
    /// for that. The `forget` call writes a rollback commit so the
    /// prior content stays in blame history.
    Remove,
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

/// Count the exact number of tokens in a string using tiktoken-rs's
/// cl100k_base encoding (GPT-3.5 / GPT-4 family). Lazily initialised;
/// the first call pays a small setup cost.
///
/// Note: Claude, Gemini, and Grok use different proprietary tokenizers.
/// cl100k_base is a widely-shared reference point and the same encoding
/// the token_savings docs reference.
fn count_tokens_cl100k(text: &str) -> usize {
    // BPE instances are cheap to clone once constructed. Cache per-thread.
    thread_local! {
        static BPE: std::cell::OnceCell<tiktoken_rs::CoreBPE> = const { std::cell::OnceCell::new() };
    }
    BPE.with(|cell| {
        let bpe = cell.get_or_init(|| tiktoken_rs::cl100k_base().expect("cl100k_base encoding"));
        bpe.encode_with_special_tokens(text).len()
    })
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
    let raw = RawCli::parse();
    let config = CtxConfig::load();
    let cli = Cli::from_raw(raw, &config);
    let client = cli.http_client();

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

            let resp = match client.clone()
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
        Commands::Recall {
            topic,
            budget,
            exact,
        } => {
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
            let mut parsed: Value = resp.json().await?;

            // If --exact was requested, re-tokenize the response body and
            // fetch the full graph to compute an exact flat baseline. Inject
            // the exact counts into the parsed JSON so emit() renders both
            // in whatever format the user wants.
            if exact {
                // Exact sent: tokenize the serialized results we already have
                let results_text = parsed
                    .get("results")
                    .map(|r| serde_json::to_string(r).unwrap_or_default())
                    .unwrap_or_default();
                let exact_sent = count_tokens_cl100k(&results_text);

                // Exact flat: fetch the full graph and tokenize it
                let flat_url = format!(
                    "{}/api/state/{}?path=/",
                    cli.server,
                    urlencoding(&cli.branch)
                );
                let exact_flat = match reqwest::get(&flat_url).await {
                    Ok(r) if r.status().is_success() => {
                        let body = r.text().await.unwrap_or_default();
                        count_tokens_cl100k(&body)
                    }
                    _ => 0,
                };

                let exact_ratio = if exact_sent > 0 {
                    exact_flat as f64 / exact_sent as f64
                } else {
                    0.0
                };

                if let Some(obj) = parsed.as_object_mut() {
                    obj.insert(
                        "ctx_tokens_sent_exact".to_string(),
                        serde_json::json!(exact_sent),
                    );
                    obj.insert(
                        "ctx_tokens_estimated_flat_exact".to_string(),
                        serde_json::json!(exact_flat),
                    );
                    obj.insert(
                        "ctx_savings_ratio_exact".to_string(),
                        serde_json::json!(exact_ratio),
                    );
                    obj.insert("tokenizer".to_string(), serde_json::json!("cl100k_base"));
                }
            }

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

                // Exact counts if requested
                if let (Some(exact_sent), Some(exact_flat), Some(exact_ratio)) = (
                    v.get("ctx_tokens_sent_exact").and_then(|x| x.as_u64()),
                    v.get("ctx_tokens_estimated_flat_exact")
                        .and_then(|x| x.as_u64()),
                    v.get("ctx_savings_ratio_exact").and_then(|x| x.as_f64()),
                ) {
                    println!(
                        "  exact (cl100k_base): {} sent, {} flat, {:.1}x savings",
                        exact_sent, exact_flat, exact_ratio
                    );
                }
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
            lens,
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
            if lens {
                args.push("--lens".to_string());
            }
            args.extend(["--port".to_string(), port.to_string()]);
            args.extend(["--storage".to_string(), storage]);
            args.extend(["--path".to_string(), db_path.clone()]);

            println!("Starting CtxOne Hub on port {} (db: {})", port, db_path);
            let status = std::process::Command::new(&hub_bin).args(&args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        Commands::Demo => {
            run_demo(&cli.server, client.clone()).await?;
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

            let resp = match client.clone()
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
        Commands::Completion { shell } => {
            let mut cmd = RawCli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, name, &mut std::io::stdout());
        }
        Commands::Doctor => {
            run_doctor(&cli).await?;
        }
        Commands::Diff { ref_a, ref_b } => {
            let url = format!(
                "{}/api/diff?ref_a={}&ref_b={}",
                cli.server,
                urlencoding(&ref_a),
                urlencoding(&ref_b),
            );
            let resp = match reqwest::get(&url).await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "diff failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let empty_vec = vec![];
                let ops = v
                    .get("ops")
                    .and_then(|x| x.as_array())
                    .unwrap_or(&empty_vec);
                if ops.is_empty() {
                    println!("No differences between {} and {}", ref_a, ref_b);
                    return;
                }
                for op in ops {
                    let tag = op.get("op").and_then(|x| x.as_str()).unwrap_or("?");
                    let path = op.get("path").and_then(|x| x.as_str()).unwrap_or("");
                    let key = op.get("key").and_then(|x| x.as_str()).unwrap_or("");
                    let marker = match tag {
                        "SetValue" => "~",
                        "AddKey" | "AppendItem" => "+",
                        "RemoveKey" | "RemoveItem" => "-",
                        _ => "?",
                    };
                    if key.is_empty() {
                        println!("{} {:12} {}", marker, tag, path);
                    } else {
                        println!("{} {:12} {}/{}", marker, tag, path, key);
                    }
                }
                println!(
                    "\n{} change{}",
                    ops.len(),
                    if ops.len() == 1 { "" } else { "s" }
                );
            });
        }
        Commands::Merge {
            source,
            into,
            message,
        } => {
            let mut body = serde_json::json!({
                "source": source,
                "target": into,
            });
            if let Some(m) = message {
                body["description"] = serde_json::json!(m);
            }

            let resp = match client.clone()
                .post(format!("{}/api/merge", cli.server))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };

            // Conflicts come back as 409 with a JSON body, not an opaque error
            if resp.status() == reqwest::StatusCode::CONFLICT {
                let text = resp.text().await?;
                if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                    emit(cli.format, &parsed, |v| {
                        let empty_vec = vec![];
                        let conflicts = v
                            .get("conflicts")
                            .and_then(|x| x.as_array())
                            .unwrap_or(&empty_vec);
                        eprintln!(
                            "Merge conflict: {} conflict{}",
                            conflicts.len(),
                            if conflicts.len() == 1 { "" } else { "s" }
                        );
                        for c in conflicts {
                            eprintln!("  {}", serde_json::to_string_pretty(c).unwrap_or_default());
                        }
                    });
                } else {
                    eprintln!("Merge conflict: {}", text);
                }
                std::process::exit(EX_DATAERR);
            }

            if !resp.status().is_success() {
                http_error_exit(resp, "merge failed").await;
            }

            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let commit = v.get("commit_id").and_then(|x| x.as_str()).unwrap_or("");
                println!("Merged '{}' into '{}' at {}", source, into, commit);
            });
        }
        Commands::Forget { path, reason } => {
            let mut body = serde_json::json!({
                "path": path.clone(),
                "ref": cli.branch,
            });
            if let Some(r) = reason {
                body["reason"] = serde_json::json!(r);
            }

            let resp = match client.clone()
                .post(format!("{}/api/memory/forget", cli.server))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "forget failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let id = v.get("commit_id").and_then(|x| x.as_str()).unwrap_or("");
                println!("Forgot: {}", path);
                println!("  commit: {}", id);
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
            let resp = match client.clone()
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
        Commands::Config { action } => {
            handle_config(action, cli.format)?;
        }
        Commands::Tokens { text } => {
            let content = if text == "-" {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("Failed to read stdin: {}", e);
                    std::process::exit(EX_IOERR);
                }
                buf
            } else {
                text
            };

            let exact = count_tokens_cl100k(&content);
            let estimate = content.len() / 4;

            let out = serde_json::json!({
                "chars": content.len(),
                "tokens_exact": exact,
                "tokens_estimate_4char": estimate,
                "tokenizer": "cl100k_base",
            });

            emit(cli.format, &out, |_| {
                println!("{} chars", content.len());
                println!("{} tokens (cl100k_base, exact)", exact);
                println!("{} tokens (4-char estimate)", estimate);
            });
        }
        Commands::Init {
            global,
            project: _,
            tool,
            config_path,
            dry_run,
            no_agents,
        } => {
            // Grab the fields agents_install_prompt needs BEFORE the
            // match consumes `cli.command` via destructuring. We only
            // need server + branch + format for the Agents handlers,
            // and the other arms don't touch them.
            let server = cli.server.clone();
            let branch = cli.branch.clone();
            let format = cli.format;
            init_mcp(global, tool, config_path, dry_run)?;
            // After MCP configs are written, optionally prime the
            // AGENTS.md guidance into the Hub. Skipped in --dry-run
            // (we don't want a dry run to actually write to the
            // graph) and when the user passed --no-agents.
            if !dry_run && !no_agents {
                println!();
                if let Err(e) = agents_install_prompt(&server, &branch, format, client.clone()).await {
                    eprintln!("  \u{2717} agents: {}", e);
                }
            }
        }
        Commands::Agents { action } => {
            let server = cli.server.clone();
            let branch = cli.branch.clone();
            let format = cli.format;
            handle_agents(action, &server, &branch, format, client.clone()).await?;
        }
        Commands::Plan { action } => {
            let server = cli.server.clone();
            let branch = cli.branch.clone();
            let format = cli.format;
            handle_plan(action, &server, &branch, format, client.clone()).await?;
        }
    }

    Ok(())
}

// -- Agents command implementation --

/// Dispatch for `ctx agents <subcommand>`. Takes server/branch/format
/// directly (not &Cli) so callers can grab these fields before the
/// outer match consumes `cli.command`.
async fn handle_agents(
    action: AgentsAction,
    server: &str,
    branch: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        AgentsAction::Show => agents_show()?,
        AgentsAction::Status => agents_status(server, branch, format, client.clone()).await?,
        AgentsAction::Remove => agents_remove(server, branch, client.clone()).await?,
        AgentsAction::Install { file, yes, show } => {
            agents_install(file, yes, show, server, branch, client.clone()).await?;
        }
    }
    Ok(())
}

/// Print the AGENTS.md content currently in effect — from disk if
/// the user has edited their local copy, otherwise the embedded
/// default — prefixed with which source it came from.
fn agents_show() -> Result<(), Box<dyn std::error::Error>> {
    let (content, display_source) =
        load_agents_md(None).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    println!("# AGENTS.md source: {}", display_source);
    println!();
    println!("{}", content);
    Ok(())
}

/// Report whether AGENTS.md has been primed in the Hub's memory graph.
/// Queries `/api/state/<ref>/paths?prefix=/memory/pinned/ctxone-agents`
/// and reports a section count and the local disk file path.
async fn agents_status(
    server: &str,
    branch: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let disk_path = agents_md_path();
    let disk_status = if disk_path.exists() {
        format!("present at {}", disk_path.display())
    } else {
        "not written (will use embedded default)".to_string()
    };

    let url = format!(
        "{}/api/state/{}/paths?prefix=/memory/pinned/{}",
        server, branch, AGENTS_SOURCE
    );
    let resp = match reqwest::get(&url).await {
        Ok(r) => r,
        Err(e) => unreachable_exit(server, e),
    };
    if !resp.status().is_success() {
        http_error_exit(resp, "status query failed").await;
    }
    let paths: Vec<String> = resp.json().await?;
    let section_count = paths
        .iter()
        .filter(|p| p.ends_with("/title") || p.ends_with("/body"))
        .count()
        / 2;

    emit(
        format,
        &serde_json::json!({
            "disk_path": disk_path.display().to_string(),
            "disk_exists": disk_path.exists(),
            "primed_sections": section_count,
            "ref": branch,
            "source": AGENTS_SOURCE,
        }),
        |_| {
            println!("AGENTS.md status");
            println!("  Disk:     {}", disk_status);
            if section_count > 0 {
                println!(
                    "  Primed:   {} sections under /memory/pinned/{}",
                    section_count, AGENTS_SOURCE
                );
                println!("  Branch:   {}", branch);
                println!();
                println!("  Inspect:  ctx ls /memory/pinned/{}", AGENTS_SOURCE);
                println!(
                    "            ctx blame /memory/pinned/{}/<slug>/body",
                    AGENTS_SOURCE
                );
            } else {
                println!("  Primed:   NOT primed on branch {}", branch);
                println!();
                println!("  Run `ctx agents install` to prime the guidance.");
            }
        },
    );
    Ok(())
}

/// Remove the primed AGENTS.md from the Hub (via forget). Does not
/// touch the local file. Blame history preserves the prior content.
async fn agents_remove(server: &str, branch: &str, client: reqwest::Client) -> Result<(), Box<dyn std::error::Error>> {
    let paths_url = format!(
        "{}/api/state/{}/paths?prefix=/memory/pinned/{}",
        server, branch, AGENTS_SOURCE
    );
    let paths_resp = match reqwest::get(&paths_url).await {
        Ok(r) => r,
        Err(e) => unreachable_exit(server, e),
    };
    if !paths_resp.status().is_success() {
        http_error_exit(paths_resp, "list paths failed").await;
    }
    let paths: Vec<String> = paths_resp.json().await?;

    if paths.is_empty() {
        println!(
            "AGENTS.md is not primed on branch {}. Nothing to remove.",
            branch
        );
        return Ok(());
    }

    let mut forgotten = 0usize;
    for path in &paths {
        let body = serde_json::json!({
            "path": path,
            "reason": "ctx agents remove",
            "ref": branch,
        });
        let resp = client
            .post(format!("{}/api/memory/forget", server))
            .json(&body)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => forgotten += 1,
            Ok(r) => {
                eprintln!("  \u{2717} forget {}: HTTP {}", path, r.status());
            }
            Err(e) => unreachable_exit(server, e),
        }
    }

    println!(
        "Removed {} AGENTS.md path(s) from /memory/pinned/{} on branch {}.",
        forgotten, AGENTS_SOURCE, branch
    );
    println!(
        "The local file at {} is untouched.",
        agents_md_path().display()
    );
    println!("Prior content is still in blame history.");
    Ok(())
}

/// The interactive install flow. Writes the file to disk (if absent),
/// shows the content unless `--yes`, prompts for confirmation, and
/// primes the sections via the Hub's prime endpoint.
async fn agents_install(
    file: Option<String>,
    yes: bool,
    show: bool,
    server: &str,
    branch: &str,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let (content, source_desc) =
        load_agents_md(file.as_deref()).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    if show {
        // Preview mode — dump the content and exit without priming.
        println!("# AGENTS.md preview");
        println!("# Source: {}", source_desc);
        println!("# (use `ctx agents install` without --show to prime it)");
        println!();
        println!("{}", content);
        return Ok(());
    }

    // Persist to disk if the user doesn't already have a local copy,
    // so subsequent `ctx agents show` reads from the editable file
    // rather than the frozen embedded default.
    let disk_path = write_agents_md_if_absent(&content)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    if !yes {
        println!("CTXone ships a short guidance file that teaches AI tools how");
        println!("to use the Hub effectively. It will be pinned to your memory");
        println!("graph so every recall response includes it.");
        println!();
        println!("  File:          {}", disk_path.display());
        println!("  Primed under:  /memory/pinned/{}", AGENTS_SOURCE);
        println!("  Branch:        {}", branch);
        println!("  Visible in:    ctx ls /memory/pinned/{}", AGENTS_SOURCE);
        println!("                 ctx blame <path>");
        println!("                 CTXone Lens browse view");
        println!("  Removable:     ctx agents remove");
        println!();
        print!("Prime AGENTS.md now? [Y/n/show] ");
        use std::io::Write;
        std::io::stdout().flush().ok();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let answer = input.trim().to_lowercase();
        match answer.as_str() {
            "" | "y" | "yes" => {}
            "show" => {
                println!();
                println!("--- AGENTS.md ---");
                println!("{}", content);
                println!("--- end ---");
                println!();
                print!("Prime AGENTS.md now? [Y/n] ");
                std::io::stdout().flush().ok();
                let mut again = String::new();
                std::io::stdin().read_line(&mut again)?;
                let a = again.trim().to_lowercase();
                if !(a.is_empty() || a == "y" || a == "yes") {
                    println!("Skipped. Run `ctx agents install` later to prime.");
                    return Ok(());
                }
            }
            _ => {
                println!("Skipped. Run `ctx agents install` later to prime.");
                return Ok(());
            }
        }
    }

    // Parse into sections and prime.
    let sections = parse_markdown_sections(&content);
    if sections.is_empty() {
        eprintln!("AGENTS.md has no H1/H2 sections — nothing to prime.");
        std::process::exit(EX_DATAERR);
    }

    let body = serde_json::json!({
        "source": AGENTS_SOURCE,
        "pinned": true,
        "sections": sections,
        "ref": branch,
    });

    let resp = match client.clone()
        .post(format!("{}/api/memory/prime", server))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => unreachable_exit(server, e),
    };
    if !resp.status().is_success() {
        http_error_exit(resp, "prime failed").await;
    }
    let parsed: Value = resp.json().await?;
    let count = parsed
        .get("sections_written")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);

    println!(
        "\u{2713} Primed {} AGENTS.md sections under /memory/pinned/{}",
        count, AGENTS_SOURCE
    );
    println!("  Disk file: {}", disk_path.display());
    println!("  Edit the file then re-run `ctx agents install` to update.");
    Ok(())
}

/// Called from the end of `ctx init` to optionally prime AGENTS.md.
/// Wraps `agents_install` in a brief summary header so the user knows
/// why the prompt is showing up after the MCP config step.
async fn agents_install_prompt(
    server: &str,
    branch: &str,
    _format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("---");
    println!();
    agents_install(None, false, false, server, branch, client).await
}

fn urlencoding(s: &str) -> String {
    s.replace(' ', "%20")
        .replace('&', "%26")
        .replace('?', "%3F")
}

fn find_hub_binary() -> String {
    let exe_suffix = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    let hub_name = format!("ctxone-hub{}", exe_suffix);
    let engine_name = format!("agentstategraph-mcp{}", exe_suffix);

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Same directory as the current `ctx` executable.
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(&hub_name));
    }

    // 2. User-local install dirs (cross-platform via dirs crate).
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local").join("bin").join(&hub_name));
        candidates.push(home.join(".local").join("bin").join(&engine_name));
    }
    if let Some(exe_dir) = dirs::executable_dir() {
        // On Windows this is typically %LOCALAPPDATA%\Microsoft\WindowsApps
        // which isn't useful for self-installed binaries, but harmless to
        // check.
        candidates.push(exe_dir.join(&hub_name));
    }

    // 3. Platform system install dirs.
    #[cfg(unix)]
    {
        candidates.push(PathBuf::from("/usr/local/bin").join(&hub_name));
        candidates.push(PathBuf::from("/usr/local/bin").join(&engine_name));
    }
    #[cfg(windows)]
    {
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates.push(PathBuf::from(program_files).join("ctxone").join(&hub_name));
        }
    }

    for c in &candidates {
        if c.exists() {
            return c.to_string_lossy().to_string();
        }
    }

    // Fall back to PATH lookup — the OS will find it if it's there.
    hub_name
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

/// Cross-platform "user data dir for app X".
///
/// On Linux: `~/.config/<app>` or `~/.<app>` depending on the tool
/// (we use `dirs::config_dir()` which is XDG-aware).
/// On macOS: `~/Library/Application Support/<app>`.
/// On Windows: `%APPDATA%\<app>`.
fn app_data_dir(app: &str) -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(app))
}

/// Cross-platform "dotfile dir" — `~/.<name>` on every platform.
/// Tools like Cursor, Gemini, Grok, Codex use this convention regardless
/// of OS because they predate or ignore XDG/AppData.
fn dotfile_dir(name: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(format!(".{}", name)))
}

fn detect_tools(global: bool) -> Vec<AiTool> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut tools: Vec<AiTool> = Vec::new();

    // ---- Claude Code ----
    // Project-level .mcp.json by default, user-level settings.json with
    // --global. Works the same on every platform.
    let claude_code_path = if global {
        dotfile_dir("claude")
            .map(|d| d.join("settings.json"))
            .unwrap_or_else(|| cwd.join(".mcp.json"))
    } else {
        cwd.join(".mcp.json")
    };
    tools.push(AiTool {
        name: "Claude Code",
        detected: true, // always available as a target
        config_path: claude_code_path,
        config_type: ConfigType::McpJson,
    });

    // ---- Claude Desktop ----
    // macOS: ~/Library/Application Support/Claude/claude_desktop_config.json
    // Windows: %APPDATA%\Claude\claude_desktop_config.json
    // Linux: ~/.config/Claude/claude_desktop_config.json (if they ever
    // release a Linux build; currently a no-op but future-proof)
    let claude_desktop = app_data_dir("Claude").map(|d| d.join("claude_desktop_config.json"));
    if let Some(path) = claude_desktop {
        tools.push(AiTool {
            name: "Claude Desktop",
            detected: path.exists(),
            config_path: path,
            config_type: ConfigType::McpJson,
        });
    }

    // ---- Cursor ----
    // ~/.cursor/mcp.json (global) or .cursor/mcp.json (project).
    let cursor_global_dir = dotfile_dir("cursor");
    let cursor_path = if global {
        cursor_global_dir
            .as_ref()
            .map(|d| d.join("mcp.json"))
            .unwrap_or_else(|| cwd.join(".cursor/mcp.json"))
    } else {
        cwd.join(".cursor/mcp.json")
    };
    let cursor_detected = cursor_global_dir.as_ref().is_some_and(|d| d.exists());
    tools.push(AiTool {
        name: "Cursor",
        detected: cursor_detected,
        config_path: cursor_path,
        config_type: ConfigType::McpJson,
    });

    // ---- VS Code ----
    // Project: .vscode/mcp.json
    // Global: platform-specific user settings.json
    //   macOS:   ~/Library/Application Support/Code/User/settings.json
    //   Windows: %APPDATA%\Code\User\settings.json
    //   Linux:   ~/.config/Code/User/settings.json
    let vscode_app_dir = app_data_dir("Code");
    let vscode_path = if global {
        vscode_app_dir
            .as_ref()
            .map(|d| d.join("User").join("settings.json"))
            .unwrap_or_else(|| cwd.join(".vscode/mcp.json"))
    } else {
        cwd.join(".vscode/mcp.json")
    };
    let vscode_detected = vscode_app_dir.as_ref().is_some_and(|d| d.exists());
    tools.push(AiTool {
        name: "VS Code",
        detected: vscode_detected,
        config_path: vscode_path,
        config_type: ConfigType::McpJson,
    });

    // ---- Codex (OpenAI CLI) ----
    // ~/.codex/config.toml on every platform
    let codex_dir = dotfile_dir("codex");
    if let Some(dir) = codex_dir {
        tools.push(AiTool {
            name: "Codex",
            detected: dir.exists(),
            config_path: dir.join("config.toml"),
            config_type: ConfigType::Toml,
        });
    }

    // ---- Gemini CLI (Google) ----
    // ~/.gemini/settings.json (global) or .gemini/settings.json (project).
    // JSON shape with mcpServers (same as Claude).
    let gemini_global = dotfile_dir("gemini");
    let gemini_path = if global {
        gemini_global
            .as_ref()
            .map(|d| d.join("settings.json"))
            .unwrap_or_else(|| cwd.join(".gemini/settings.json"))
    } else {
        cwd.join(".gemini/settings.json")
    };
    let gemini_detected = gemini_global.as_ref().is_some_and(|d| d.exists());
    tools.push(AiTool {
        name: "Gemini",
        detected: gemini_detected,
        config_path: gemini_path,
        config_type: ConfigType::McpJson,
    });

    // ---- Grok CLI (xAI / superagent-ai/grok-cli) ----
    // Same pattern as Gemini.
    let grok_global = dotfile_dir("grok");
    let grok_path = if global {
        grok_global
            .as_ref()
            .map(|d| d.join("settings.json"))
            .unwrap_or_else(|| cwd.join(".grok/settings.json"))
    } else {
        cwd.join(".grok/settings.json")
    };
    let grok_detected = grok_global.as_ref().is_some_and(|d| d.exists());
    tools.push(AiTool {
        name: "Grok",
        detected: grok_detected,
        config_path: grok_path,
        config_type: ConfigType::McpJson,
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

async fn run_doctor(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut suggestions: Vec<String> = Vec::new();

    // Check 1: hub binary discoverable
    let hub_bin = find_hub_binary();
    let hub_exists = std::path::Path::new(&hub_bin).exists();
    checks.push((
        "ctxone-hub binary".to_string(),
        hub_exists,
        if hub_exists {
            hub_bin.clone()
        } else {
            "not found in PATH or ~/.local/bin".to_string()
        },
    ));
    if !hub_exists {
        suggestions.push(
            "Install ctxone-hub: curl -sSL https://raw.githubusercontent.com/ctxone/ctxone/main/install.sh | sh".to_string(),
        );
    }

    // Check 2: canonical db path writable
    let db = canonical_db_path();
    let db_path = std::path::Path::new(&db);
    let parent = db_path.parent().map(|p| p.to_path_buf());
    let parent_ok = parent
        .as_ref()
        .map(|p| p.exists() || std::fs::create_dir_all(p).is_ok())
        .unwrap_or(false);
    checks.push(("memory db location".to_string(), parent_ok, db.clone()));
    if !parent_ok {
        suggestions.push(format!(
            "Create the db directory: mkdir -p {}",
            parent.as_ref().and_then(|p| p.to_str()).unwrap_or("")
        ));
    }

    // Check 3: Hub HTTP reachable
    let hub_reachable = reqwest::get(format!("{}/api/health", cli.server))
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    checks.push((
        "hub HTTP endpoint".to_string(),
        hub_reachable,
        cli.server.clone(),
    ));
    if !hub_reachable {
        suggestions.push(format!(
            "Start the Hub: ctx serve --http  (or set CTX_SERVER={})",
            cli.server
        ));
    }

    // Check 4: memory branch exists
    let main_exists = if hub_reachable {
        reqwest::get(format!("{}/api/stats/main", cli.server))
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    } else {
        false
    };
    checks.push((
        "main branch".to_string(),
        main_exists,
        if main_exists {
            "reachable".to_string()
        } else {
            "not reachable (hub down?)".to_string()
        },
    ));

    // Check 5: MCP configs for detected AI tools
    let tools = detect_tools(false);
    for t in &tools {
        if !t.detected {
            continue;
        }
        let has_mcp = if t.config_path.exists() {
            match std::fs::read_to_string(&t.config_path) {
                Ok(content) => content.contains("\"ctxone\""),
                Err(_) => false,
            }
        } else {
            false
        };
        checks.push((
            format!("{} MCP config", t.name),
            has_mcp,
            if has_mcp {
                format!("configured at {}", t.config_path.display())
            } else {
                "ctxone not in mcpServers".to_string()
            },
        ));
        if !has_mcp {
            suggestions.push(format!(
                "Configure {}: ctx init --tool {}",
                t.name,
                t.name
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_lowercase()
            ));
        }
    }

    // Build structured output
    let checks_json: Vec<Value> = checks
        .iter()
        .map(|(name, ok, detail)| {
            serde_json::json!({
                "name": name,
                "ok": ok,
                "detail": detail,
            })
        })
        .collect();
    let all_ok = checks.iter().all(|(_, ok, _)| *ok);
    let out = serde_json::json!({
        "all_ok": all_ok,
        "checks": checks_json,
        "suggestions": suggestions,
    });

    emit(cli.format, &out, |_| {
        println!("CtxOne Doctor");
        println!();
        for (name, ok, detail) in &checks {
            let marker = if *ok { "\u{2713}" } else { "\u{2717}" };
            println!("  {} {:24} {}", marker, name, detail);
        }
        println!();
        if all_ok {
            println!("All checks passed.");
        } else {
            println!("Suggestions:");
            for s in &suggestions {
                println!("  \u{2192} {}", s);
            }
        }
    });

    if !all_ok {
        std::process::exit(EX_SOFTWARE);
    }
    Ok(())
}

async fn run_demo(server: &str, client: reqwest::Client) -> Result<(), Box<dyn std::error::Error>> {
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

/// Path where the user's editable AGENTS.md lives.
///
/// On Unix this is `~/.config/ctxone/AGENTS.md`. On Windows this is
/// `%APPDATA%\ctxone\AGENTS.md`. Falls back to `./AGENTS.md` only if
/// the dirs crate can't resolve a config dir, which should never
/// happen in practice.
fn agents_md_path() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        dirs::config_dir().or_else(dirs::data_dir)
    } else {
        dirs::config_dir()
    };
    match base {
        Some(dir) => dir.join("ctxone").join("AGENTS.md"),
        None => PathBuf::from("./AGENTS.md"),
    }
}

/// Read AGENTS.md from disk if it exists, otherwise return the
/// compile-time embedded default. The disk file takes precedence
/// so user edits survive upgrades — the only way to revert to the
/// embedded default is to delete the local file.
fn load_agents_md(override_path: Option<&str>) -> Result<(String, String), String> {
    let path = match override_path {
        Some(p) => PathBuf::from(p),
        None => agents_md_path(),
    };

    if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        Ok((content, path.display().to_string()))
    } else {
        Ok((
            EMBEDDED_AGENTS_MD.to_string(),
            format!("(embedded default — will be written to {})", path.display()),
        ))
    }
}

/// Write AGENTS.md to disk if no local copy exists yet. Does NOT
/// overwrite an existing file — user edits are sacred.
fn write_agents_md_if_absent(content: &str) -> Result<PathBuf, String> {
    let path = agents_md_path();
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(path)
}

/// Canonical path where CTXone stores its default memory database.
///
/// On Unix this is `~/.ctxone/memory.db`. On Windows this is
/// `%APPDATA%\ctxone\memory.db` (typically
/// `C:\Users\<you>\AppData\Roaming\ctxone\memory.db`).
///
/// Falls back to `./ctxone.db` if we can't determine a home directory,
/// which should never happen in practice.
fn canonical_db_path() -> String {
    let base = if cfg!(target_os = "windows") {
        dirs::data_dir()
    } else {
        dirs::home_dir().map(|h| h.join(".ctxone"))
    };

    match base {
        Some(dir) => {
            let p = if cfg!(target_os = "windows") {
                dir.join("ctxone").join("memory.db")
            } else {
                dir.join("memory.db")
            };
            p.to_string_lossy().into_owned()
        }
        None => "./ctxone.db".to_string(),
    }
}

/// Convert a display name like "Claude Code" into an agent-ID slug
/// like "claude-code". Used to stamp ctx blame output with the
/// originating AI tool.
fn tool_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn mcp_server_entry(agent_id: &str) -> Value {
    let hub_bin = find_hub_binary();
    let db_path = canonical_db_path();

    // Ensure the parent directory exists so the Hub can create the db on first run.
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    serde_json::json!({
        "command": hub_bin,
        "args": ["--path", db_path, "--agent-id", agent_id]
    })
}

/// Merge a `[mcp_servers.ctxone]` entry into an existing Codex TOML config.
///
/// Preserves all existing keys and sections. If the ctxone entry already
/// exists, it's overwritten with the new command and args. Other mcp_servers
/// entries (linear, figma, etc.) are left untouched.
///
/// Returns the serialized TOML ready to write.
fn merge_codex_ctxone_toml(
    existing: &str,
    hub_bin: &str,
    db_path: &str,
    agent_id: &str,
) -> Result<String, String> {
    use toml::Value;

    // Parse existing content (or start with an empty table)
    let mut doc: Value = if existing.trim().is_empty() {
        Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(existing).map_err(|e| format!("invalid existing TOML: {}", e))?
    };

    // Ensure mcp_servers is a table
    let root = doc
        .as_table_mut()
        .ok_or_else(|| "config root is not a table".to_string())?;

    let servers = root
        .entry("mcp_servers".to_string())
        .or_insert_with(|| Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| "mcp_servers is not a table".to_string())?;

    // Build the ctxone entry
    let mut ctxone = toml::map::Map::new();
    ctxone.insert("command".to_string(), Value::String(hub_bin.to_string()));
    ctxone.insert(
        "args".to_string(),
        Value::Array(vec![
            Value::String("--path".to_string()),
            Value::String(db_path.to_string()),
            Value::String("--agent-id".to_string()),
            Value::String(agent_id.to_string()),
        ]),
    );

    servers.insert("ctxone".to_string(), Value::Table(ctxone));

    toml::to_string_pretty(&doc).map_err(|e| format!("serialize failed: {}", e))
}

fn handle_config(
    action: ConfigAction,
    format: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        ConfigAction::Path => {
            emit(
                format,
                &serde_json::json!({ "path": CtxConfig::path() }),
                |_| {
                    println!("{}", CtxConfig::path().display());
                },
            );
        }
        ConfigAction::Show => {
            let config = CtxConfig::load();
            let value = serde_json::to_value(&config).unwrap_or_default();
            emit(format, &value, |v| {
                if v.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    println!("(empty config at {})", CtxConfig::path().display());
                    println!("Set a value with: ctx config set <key> <value>");
                } else {
                    println!("Config file: {}", CtxConfig::path().display());
                    if let Some(s) = v.get("server").and_then(|x| x.as_str()) {
                        println!("  server: {}", s);
                    }
                    if let Some(b) = v.get("branch").and_then(|x| x.as_str()) {
                        println!("  branch: {}", b);
                    }
                    if let Some(f) = v.get("format").and_then(|x| x.as_str()) {
                        println!("  format: {}", f);
                    }
                }
            });
        }
        ConfigAction::Get { key } => {
            let config = CtxConfig::load();
            match config.get_key(&key) {
                Ok(value) => {
                    emit(
                        format,
                        &serde_json::json!({ "key": key, "value": value }),
                        |_| println!("{}", value),
                    );
                }
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(EX_DATAERR);
                }
            }
        }
        ConfigAction::Set { key, value } => {
            let mut config = CtxConfig::load();
            if let Err(e) = config.set_key(&key, &value) {
                eprintln!("{}", e);
                std::process::exit(EX_DATAERR);
            }
            config.save()?;
            emit(
                format,
                &serde_json::json!({ "status": "ok", "key": key, "value": value }),
                |_| {
                    println!("Saved: {} = {}", key, value);
                    println!("  → {}", CtxConfig::path().display());
                },
            );
        }
        ConfigAction::Unset { key } => {
            let mut config = CtxConfig::load();
            if let Err(e) = config.unset_key(&key) {
                eprintln!("{}", e);
                std::process::exit(EX_DATAERR);
            }
            config.save()?;
            emit(
                format,
                &serde_json::json!({ "status": "ok", "key": key }),
                |_| println!("Unset: {}", key),
            );
        }
    }
    Ok(())
}

fn init_mcp(
    global: bool,
    tool_filter: Option<String>,
    generic_config_path: Option<String>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tools = detect_tools(global);

    // Generic fallback: if the user passed --config-path, add a synthetic
    // tool entry pointing at that path. Treated as McpJson (the de-facto
    // standard for MCP client config files).
    if let Some(path_str) = generic_config_path.as_deref() {
        tools.push(AiTool {
            name: "Generic",
            detected: true,
            config_path: PathBuf::from(path_str),
            config_type: ConfigType::McpJson,
        });
    }

    // If --config-path was supplied without --tool, narrow the filter to
    // just the Generic entry so we don't also install into every detected
    // tool by accident.
    let effective_filter = match (&tool_filter, &generic_config_path) {
        (None, Some(_)) => Some("Generic".to_string()),
        _ => tool_filter.clone(),
    };

    println!("Detected AI tools:");
    for t in &tools {
        let icon = if t.detected { "\u{2713}" } else { "\u{2717}" };
        println!("  {} {}", icon, t.name);
    }
    println!();

    // When the user asks for a specific tool by name, skip the "detected"
    // gate — they know they want it, even if the tool isn't currently
    // installed. This lets users set up configs ahead of installing a
    // tool, and avoids the "why isn't my --tool codex working" footgun.
    let targets: Vec<&AiTool> = tools
        .iter()
        .filter(|t| {
            if let Some(f) = effective_filter.as_ref() {
                t.name.to_lowercase().contains(&f.to_lowercase())
            } else {
                t.detected
            }
        })
        .collect();

    if targets.is_empty() {
        println!("No matching AI tools detected.");
        return Ok(());
    }

    for t in &targets {
        // Per-tool agent ID: a slug of the detected tool name
        // (e.g. "Claude Code" → "claude-code"). This makes
        // `ctx blame` show the originating tool for every commit.
        let agent_id = tool_slug(t.name);
        let entry = mcp_server_entry(&agent_id);

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
                let hub_bin = find_hub_binary();
                let db_path = canonical_db_path();
                if let Some(parent) = std::path::Path::new(&db_path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                let existing = if t.config_path.exists() {
                    std::fs::read_to_string(&t.config_path).unwrap_or_default()
                } else {
                    String::new()
                };

                let new_content =
                    match merge_codex_ctxone_toml(&existing, &hub_bin, &db_path, &agent_id) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("  \u{2717} {}: could not merge TOML config: {}", t.name, e);
                            continue;
                        }
                    };

                if dry_run {
                    println!(
                        "  [dry-run] {}: would write {}",
                        t.name,
                        t.config_path.display()
                    );
                    for line in new_content.lines() {
                        println!("    {}", line);
                    }
                } else {
                    if let Some(parent) = t.config_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&t.config_path, new_content)?;
                    println!(
                        "  \u{2192} {}: wrote {} \u{2713}",
                        t.name,
                        t.config_path.display()
                    );
                }
            }
        }
    }

    println!();
    println!("CtxOne is ready. Try: \"remember that we use BSL-1.1 licensing\"");

    Ok(())
}

// -- Plan command implementation ------------------------------------

/// Render the task status with a compact glyph for TTY output.
fn status_glyph(status: &str) -> &'static str {
    match status {
        "done" => "[x]",
        "in_progress" => "[>]",
        "abandoned" => "[!]",
        _ => "[ ]",
    }
}

fn priority_tag(priority: &str) -> &'static str {
    match priority {
        "critical" => "[CR]",
        "high" => "[HI]",
        "medium" => "[ME]",
        "low" => "[LO]",
        _ => "[??]",
    }
}

async fn handle_plan(
    action: PlanAction,
    server: &str,
    branch: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let agent_id = std::env::var("CTX_AGENT_ID").unwrap_or_else(|_| "ctx-cli".to_string());

    match action {
        PlanAction::New { name, description } => {
            let mut body = serde_json::json!({
                "name": name.clone(),
                "ref": branch,
            });
            if let Some(d) = description {
                body["description"] = serde_json::json!(d);
            }
            let resp = match client
                .post(format!("{}/api/plans", server))
                .header("X-CTXone-Agent", &agent_id)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan new failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                println!("Plan created: {}", v["name"].as_str().unwrap_or(""));
                if let Some(s) = v["status"].as_str() {
                    println!("  status: {}", s);
                }
            });
        }
        PlanAction::Add {
            plan_id,
            title,
            description,
            priority,
            parent,
            assigned_to,
            blocks,
        } => {
            let mut body = serde_json::json!({
                "title": title,
                "priority": priority,
                "ref": branch,
            });
            if let Some(d) = description {
                body["description"] = serde_json::json!(d);
            }
            if let Some(p) = parent {
                body["parent_id"] = serde_json::json!(p);
            }
            if let Some(a) = assigned_to {
                body["assigned_to"] = serde_json::json!(a);
            }
            if !blocks.is_empty() {
                body["blocked_by"] = serde_json::json!(blocks);
            }
            let url = format!("{}/api/plans/{}/tasks", server, urlencoding(&plan_id));
            let resp = match client
                .post(&url)
                .header("X-CTXone-Agent", &agent_id)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan add failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let id = v["id"].as_str().unwrap_or("?");
                let title = v["title"].as_str().unwrap_or("");
                let pri = v["priority"].as_str().unwrap_or("medium");
                println!("Added {} {} {}", id, priority_tag(pri), title);
                if let Some(a) = v["assigned_to"].as_str() {
                    println!("  assigned to: {}", a);
                }
                if let Some(blockers) = v["blocked_by"].as_array()
                    && !blockers.is_empty()
                {
                    let list: Vec<String> = blockers
                        .iter()
                        .filter_map(|b| b.as_str().map(String::from))
                        .collect();
                    println!("  blocked by: {}", list.join(", "));
                }
            });
        }
        PlanAction::Start {
            plan_id,
            task_id,
            reason,
        } => {
            let body = match reason {
                Some(r) => serde_json::json!({"reason": r, "ref": branch}),
                None => serde_json::json!({"ref": branch}),
            };
            let url = format!(
                "{}/api/plans/{}/tasks/{}/start",
                server,
                urlencoding(&plan_id),
                urlencoding(&task_id),
            );
            let resp = match client
                .post(&url)
                .header("X-CTXone-Agent", &agent_id)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan start failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                println!(
                    "Started {}: {}",
                    v["id"].as_str().unwrap_or(""),
                    v["title"].as_str().unwrap_or("")
                );
                println!("  status: {}", v["status"].as_str().unwrap_or("?"));
            });
        }
        PlanAction::Done {
            plan_id,
            task_id,
            proof,
            reason,
        } => {
            let (kind, value, note) = match parse_proof_spec(&proof) {
                Ok(triple) => triple,
                Err(e) => {
                    eprintln!("plan done: {}", e);
                    std::process::exit(EX_DATAERR);
                }
            };
            let mut proof_obj = serde_json::json!({"kind": kind, "value": value});
            if let Some(n) = note {
                proof_obj["note"] = serde_json::json!(n);
            }
            let mut body = serde_json::json!({"proof": proof_obj, "ref": branch});
            if let Some(r) = reason {
                body["reason"] = serde_json::json!(r);
            }
            let url = format!(
                "{}/api/plans/{}/tasks/{}/complete",
                server,
                urlencoding(&plan_id),
                urlencoding(&task_id),
            );
            let resp = match client
                .post(&url)
                .header("X-CTXone-Agent", &agent_id)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan done failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                println!(
                    "Marked {} done: {}",
                    v["id"].as_str().unwrap_or(""),
                    v["title"].as_str().unwrap_or("")
                );
                if let Some(p) = v.get("proof") {
                    let kind = p["kind"].as_str().unwrap_or("?");
                    let val = p["value"].as_str().unwrap_or("");
                    println!("  proof: {} {}", kind, val);
                }
            });
        }
        PlanAction::Abandon {
            plan_id,
            task_id,
            reason,
        } => {
            let body = serde_json::json!({"reason": reason, "ref": branch});
            let url = format!(
                "{}/api/plans/{}/tasks/{}/abandon",
                server,
                urlencoding(&plan_id),
                urlencoding(&task_id),
            );
            let resp = match client
                .post(&url)
                .header("X-CTXone-Agent", &agent_id)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan abandon failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                println!(
                    "Abandoned {}: {}",
                    v["id"].as_str().unwrap_or(""),
                    v["title"].as_str().unwrap_or("")
                );
                println!("  reason: {}", v["abandoned_reason"].as_str().unwrap_or(""));
            });
        }
        PlanAction::Next {
            plan_id,
            assigned_to,
            me,
            include_unassigned,
            assigned_only,
        } => {
            let mut parts = vec![format!("ref={}", urlencoding(branch))];
            let assignee = if me {
                Some(agent_id.clone())
            } else {
                assigned_to
            };
            if let Some(a) = assignee {
                parts.push(format!("assigned_to={}", urlencoding(&a)));
            }
            parts.push(format!("include_unassigned={}", include_unassigned));
            if assigned_only {
                parts.push("assigned_only=true".to_string());
            }
            let url = format!(
                "{}/api/plans/{}/next?{}",
                server,
                urlencoding(&plan_id),
                parts.join("&"),
            );
            let resp = match client
                .get(&url)
                .header("X-CTXone-Agent", &agent_id)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan next failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| match v.get("task") {
                Some(t) if !t.is_null() => {
                    let pri = t["priority"].as_str().unwrap_or("");
                    println!(
                        "Next: {} {} {}",
                        t["id"].as_str().unwrap_or(""),
                        priority_tag(pri),
                        t["title"].as_str().unwrap_or("")
                    );
                    if let Some(a) = t["assigned_to"].as_str() {
                        println!("  assigned to: {}", a);
                    }
                }
                _ => {
                    println!("No pickable tasks.");
                }
            });
        }
        PlanAction::List { status } => {
            let mut url = format!("{}/api/plans?ref={}", server, urlencoding(branch));
            if let Some(s) = status {
                url.push_str(&format!("&status={}", urlencoding(&s)));
            }
            let resp = match client
                .get(&url)
                .header("X-CTXone-Agent", &agent_id)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan list failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let empty = vec![];
                let arr = v.as_array().unwrap_or(&empty);
                if arr.is_empty() {
                    println!("No plans.");
                    return;
                }
                for plan in arr {
                    let name = plan["name"].as_str().unwrap_or("");
                    let status = plan["status"].as_str().unwrap_or("?");
                    let counts = &plan["task_counts"];
                    let done = counts["done"].as_u64().unwrap_or(0);
                    let in_progress = counts["in_progress"].as_u64().unwrap_or(0);
                    let pending = counts["pending"].as_u64().unwrap_or(0);
                    let total = counts["total"].as_u64().unwrap_or(0);
                    println!(
                        "{:<24} {:<10} {} tasks [{}✓ {}→ {} ]",
                        name, status, total, done, in_progress, pending
                    );
                }
            });
        }
        PlanAction::Show { plan_id } => {
            let url = format!(
                "{}/api/plans/{}?ref={}",
                server,
                urlencoding(&plan_id),
                urlencoding(branch)
            );
            let resp = match client
                .get(&url)
                .header("X-CTXone-Agent", &agent_id)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan show failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let name = v["name"].as_str().unwrap_or("");
                let status = v["status"].as_str().unwrap_or("?");
                let desc = v["description"].as_str().unwrap_or("");
                println!("Plan: {} — {}", name, desc);
                println!("  status: {}", status);
                let empty = vec![];
                let tasks = v["tasks"].as_array().unwrap_or(&empty);
                if tasks.is_empty() {
                    println!("  (no tasks)");
                    return;
                }
                println!();
                for task in tasks {
                    let id = task["id"].as_str().unwrap_or("");
                    let title = task["title"].as_str().unwrap_or("");
                    let status = task["status"].as_str().unwrap_or("");
                    let pri = task["priority"].as_str().unwrap_or("");
                    let assigned = task["assigned_to"].as_str();
                    let mut line = format!(
                        "  {} {} {} {}",
                        status_glyph(status),
                        id,
                        priority_tag(pri),
                        title
                    );
                    if let Some(a) = assigned {
                        line.push_str(&format!(" @{}", a));
                    }
                    println!("{}", line);
                    if let Some(proof) = task.get("proof")
                        && !proof.is_null()
                    {
                        let kind = proof["kind"].as_str().unwrap_or("?");
                        let val = proof["value"].as_str().unwrap_or("");
                        println!("      proof: {} {}", kind, val);
                    }
                    if status == "abandoned"
                        && let Some(reason) = task["abandoned_reason"].as_str()
                    {
                        println!("      reason: {}", reason);
                    }
                    let blockers: Vec<&str> = task["blocked_by"]
                        .as_array()
                        .map(|arr| arr.iter().filter_map(|b| b.as_str()).collect())
                        .unwrap_or_default();
                    if !blockers.is_empty() {
                        println!("      blocked by: {}", blockers.join(", "));
                    }
                }
            });
        }
        PlanAction::Archive { plan_id } => {
            let url = format!("{}/api/plans/{}/archive", server, urlencoding(&plan_id),);
            let resp = match client
                .post(&url)
                .header("X-CTXone-Agent", &agent_id)
                .json(&serde_json::json!({"ref": branch}))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan archive failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                println!("Archived plan {}", v["name"].as_str().unwrap_or(""));
            });
        }
    }
    Ok(())
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -------- urlencoding --------

    #[test]
    fn urlencoding_escapes_spaces() {
        assert_eq!(urlencoding("hello world"), "hello%20world");
    }

    #[test]
    fn urlencoding_escapes_ampersand_and_question() {
        assert_eq!(urlencoding("a&b?c"), "a%26b%3Fc");
    }

    #[test]
    fn urlencoding_passthrough_safe_chars() {
        assert_eq!(urlencoding("abc123"), "abc123");
    }

    // -------- parse_markdown_sections --------

    #[test]
    fn parse_markdown_sections_h1_split() {
        let md = "# First\n\nbody of first\n\n# Second\n\nbody of second\n";
        let sections = parse_markdown_sections(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "First");
        assert_eq!(sections[0].body, "body of first");
        assert_eq!(sections[1].title, "Second");
        assert_eq!(sections[1].body, "body of second");
    }

    #[test]
    fn parse_markdown_sections_h2_split() {
        let md = "## One\n\ntext one\n\n## Two\n\ntext two";
        let sections = parse_markdown_sections(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "One");
        assert_eq!(sections[1].title, "Two");
    }

    #[test]
    fn parse_markdown_sections_h3_does_not_split() {
        // H3 should be treated as body content of the enclosing H1 or H2
        let md = "# Top\n\nintro\n\n### Sub\n\ndeep content\n";
        let sections = parse_markdown_sections(md);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Top");
        assert!(sections[0].body.contains("intro"));
        assert!(sections[0].body.contains("### Sub"));
        assert!(sections[0].body.contains("deep content"));
    }

    #[test]
    fn parse_markdown_sections_intro_before_first_heading() {
        let md = "some preamble\n\nmore preamble\n\n# First\n\nbody\n";
        let sections = parse_markdown_sections(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Intro");
        assert_eq!(sections[0].body, "some preamble\n\nmore preamble");
        assert_eq!(sections[1].title, "First");
    }

    #[test]
    fn parse_markdown_sections_empty_body_skipped() {
        let md = "# Only heading\n";
        let sections = parse_markdown_sections(md);
        assert_eq!(sections.len(), 0); // no body → skipped
    }

    #[test]
    fn parse_markdown_sections_empty_input() {
        let sections = parse_markdown_sections("");
        assert_eq!(sections.len(), 0);
    }

    // -------- extract_id --------

    #[test]
    fn extract_id_prefers_name() {
        let v = json!({ "name": "main", "commit_id": "sg_abc", "id": "xyz" });
        assert_eq!(extract_id(&v), Some("main"));
    }

    #[test]
    fn extract_id_falls_back_to_commit_id() {
        let v = json!({ "commit_id": "sg_abc", "path": "/foo" });
        assert_eq!(extract_id(&v), Some("sg_abc"));
    }

    #[test]
    fn extract_id_falls_back_to_path() {
        let v = json!({ "path": "/memory/facts/123" });
        assert_eq!(extract_id(&v), Some("/memory/facts/123"));
    }

    #[test]
    fn extract_id_none_for_empty_object() {
        let v = json!({});
        assert_eq!(extract_id(&v), None);
    }

    #[test]
    fn extract_id_none_for_array() {
        let v = json!([1, 2, 3]);
        assert_eq!(extract_id(&v), None);
    }

    // -------- canonical_db_path --------

    #[test]
    fn canonical_db_path_ends_with_memory_db() {
        // The exact path varies by platform (Unix: ~/.ctxone/memory.db,
        // Windows: %APPDATA%\ctxone\memory.db), so we only check the
        // suffix and that we got a non-empty path.
        let p = canonical_db_path();
        assert!(!p.is_empty());
        assert!(
            p.ends_with("memory.db"),
            "canonical db path should end with memory.db, got: {}",
            p
        );
        assert!(
            p.contains("ctxone"),
            "canonical db path should contain 'ctxone', got: {}",
            p
        );
    }

    // -------- CtxConfig --------

    #[test]
    fn ctx_config_default_is_empty() {
        let cfg = CtxConfig::default();
        assert!(cfg.server.is_none());
        assert!(cfg.branch.is_none());
        assert!(cfg.format.is_none());
    }

    #[test]
    fn ctx_config_set_and_get_round_trip() {
        let mut cfg = CtxConfig::default();
        cfg.set_key("server", "http://example.com:3001").unwrap();
        cfg.set_key("branch", "dev").unwrap();
        cfg.set_key("format", "json").unwrap();

        assert_eq!(cfg.get_key("server").unwrap(), "http://example.com:3001");
        assert_eq!(cfg.get_key("branch").unwrap(), "dev");
        assert_eq!(cfg.get_key("format").unwrap(), "json");
    }

    #[test]
    fn ctx_config_set_rejects_unknown_key() {
        let mut cfg = CtxConfig::default();
        assert!(cfg.set_key("hostname", "foo").is_err());
    }

    #[test]
    fn ctx_config_set_rejects_invalid_format() {
        let mut cfg = CtxConfig::default();
        assert!(cfg.set_key("format", "yaml").is_err());
    }

    #[test]
    fn ctx_config_unset_clears_value() {
        let mut cfg = CtxConfig::default();
        cfg.set_key("server", "http://example.com").unwrap();
        assert_eq!(cfg.get_key("server").unwrap(), "http://example.com");

        cfg.unset_key("server").unwrap();
        assert_eq!(cfg.get_key("server").unwrap(), "");
        assert!(cfg.server.is_none());
    }

    #[test]
    fn ctx_config_toml_round_trip() {
        let mut cfg = CtxConfig::default();
        cfg.set_key("server", "http://example.com:3001").unwrap();
        cfg.set_key("format", "id").unwrap();

        let serialized = toml::to_string(&cfg).unwrap();
        let deserialized: CtxConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(
            deserialized.server.as_deref(),
            Some("http://example.com:3001")
        );
        assert_eq!(deserialized.format, Some(OutputFormat::Id));
        assert_eq!(deserialized.branch, None);
    }

    // -------- Cli::from_raw priority resolution --------

    fn raw_with_no_flags() -> RawCli {
        RawCli {
            server: None,
            branch: None,
            format: None,
            command: Commands::Status,
        }
    }

    #[test]
    fn cli_resolution_uses_hardcoded_defaults_when_nothing_set() {
        let cli = Cli::from_raw(raw_with_no_flags(), &CtxConfig::default());
        assert_eq!(cli.server, "http://localhost:3001");
        assert_eq!(cli.branch, "main");
        assert_eq!(cli.format, OutputFormat::Text);
    }

    #[test]
    fn cli_resolution_uses_config_when_no_flag_or_env() {
        let mut config = CtxConfig::default();
        config.set_key("server", "http://config:3001").unwrap();
        config.set_key("branch", "dev").unwrap();

        let cli = Cli::from_raw(raw_with_no_flags(), &config);
        assert_eq!(cli.server, "http://config:3001");
        assert_eq!(cli.branch, "dev");
        assert_eq!(cli.format, OutputFormat::Text); // config didn't set this
    }

    #[test]
    fn cli_resolution_flag_overrides_config() {
        let mut config = CtxConfig::default();
        config.set_key("server", "http://config:3001").unwrap();

        let mut raw = raw_with_no_flags();
        raw.server = Some("http://flag:3001".to_string());

        let cli = Cli::from_raw(raw, &config);
        assert_eq!(cli.server, "http://flag:3001");
    }

    // -------- count_tokens_cl100k --------

    #[test]
    fn count_tokens_empty_string() {
        assert_eq!(count_tokens_cl100k(""), 0);
    }

    #[test]
    fn count_tokens_single_word() {
        // "hello" is a single token in cl100k
        assert_eq!(count_tokens_cl100k("hello"), 1);
    }

    #[test]
    fn count_tokens_short_sentence() {
        // "The quick brown fox jumps over the lazy dog" — 9 tokens in cl100k
        // (widely-quoted reference value)
        assert_eq!(
            count_tokens_cl100k("The quick brown fox jumps over the lazy dog"),
            9
        );
    }

    #[test]
    fn count_tokens_multiple_calls_consistent() {
        // Guards against thread-local state corruption across calls
        let a = count_tokens_cl100k("CtxOne memory layer");
        let b = count_tokens_cl100k("CtxOne memory layer");
        assert_eq!(a, b);
        assert!(a > 0);
    }

    // -------- merge_codex_ctxone_toml --------

    #[test]
    fn codex_merge_creates_entry_in_empty_config() {
        let out = merge_codex_ctxone_toml(
            "",
            "/usr/local/bin/ctxone-hub",
            "/home/user/.ctxone/memory.db",
            "codex",
        )
        .expect("merge should succeed on empty input");
        assert!(out.contains("[mcp_servers.ctxone]"));
        assert!(out.contains("command = \"/usr/local/bin/ctxone-hub\""));
        assert!(out.contains("--path"));
        assert!(out.contains("/home/user/.ctxone/memory.db"));
        // New: agent-id flag should be passed through
        assert!(out.contains("--agent-id"));
        assert!(out.contains("\"codex\""));
    }

    #[test]
    fn codex_merge_preserves_other_mcp_servers() {
        let existing = r#"
[mcp_servers.linear]
command = "wsl"
args = ["npx", "-y", "mcp-remote", "https://mcp.linear.app/sse"]
"#;
        let out = merge_codex_ctxone_toml(existing, "/bin/ctxone-hub", "/db", "codex")
            .expect("merge should succeed");
        assert!(out.contains("[mcp_servers.linear]"));
        assert!(out.contains("[mcp_servers.ctxone]"));
        assert!(out.contains("\"wsl\""));
        assert!(out.contains("\"npx\""));
    }

    #[test]
    fn codex_merge_preserves_top_level_keys() {
        let existing = r#"
project_trust_level = "workspace-trusted"
some_other_setting = 42

[mcp_servers.figma]
command = "figma-mcp"
"#;
        let out = merge_codex_ctxone_toml(existing, "/bin/ctxone-hub", "/db", "codex")
            .expect("merge should succeed");
        assert!(out.contains("project_trust_level = \"workspace-trusted\""));
        assert!(out.contains("some_other_setting = 42"));
        assert!(out.contains("[mcp_servers.figma]"));
        assert!(out.contains("[mcp_servers.ctxone]"));
    }

    #[test]
    fn codex_merge_is_idempotent() {
        // First merge
        let first =
            merge_codex_ctxone_toml("", "/bin/hub", "/db/main.db", "codex").expect("first merge");
        // Second merge on the output of the first
        let second = merge_codex_ctxone_toml(&first, "/bin/hub", "/db/main.db", "codex")
            .expect("second merge");
        assert_eq!(first, second);
    }

    #[test]
    fn codex_merge_overwrites_stale_ctxone_entry() {
        let existing = r#"
[mcp_servers.ctxone]
command = "/old/path/ctxone-hub"
args = ["--path", "/old/db"]
"#;
        let out = merge_codex_ctxone_toml(existing, "/new/path/ctxone-hub", "/new/db", "codex")
            .expect("merge should succeed");
        assert!(out.contains("/new/path/ctxone-hub"));
        assert!(out.contains("/new/db"));
        assert!(!out.contains("/old/path/ctxone-hub"));
        assert!(!out.contains("/old/db"));
    }

    #[test]
    fn codex_merge_rejects_invalid_toml() {
        let broken = "this is { not valid toml }}";
        assert!(merge_codex_ctxone_toml(broken, "/bin/hub", "/db", "codex").is_err());
    }

    // -------- tool_slug --------

    #[test]
    fn tool_slug_lowercases_and_dashes() {
        assert_eq!(tool_slug("Claude Code"), "claude-code");
        assert_eq!(tool_slug("VS Code"), "vs-code");
        assert_eq!(tool_slug("Cursor"), "cursor");
    }

    #[test]
    fn tool_slug_collapses_punctuation_and_runs_of_whitespace() {
        assert_eq!(tool_slug("Claude  Code (beta)"), "claude-code-beta");
    }

    #[test]
    fn tool_slug_handles_already_slugged_names() {
        assert_eq!(tool_slug("codex"), "codex");
        assert_eq!(tool_slug("my-tool"), "my-tool");
    }

    #[test]
    fn tool_slug_trims_trailing_dashes() {
        assert_eq!(tool_slug("Tool!"), "tool");
    }

    // -------- mcp_server_entry with agent_id --------

    #[test]
    fn mcp_server_entry_includes_agent_id() {
        let entry = mcp_server_entry("claude-code");
        let args = entry
            .get("args")
            .and_then(|v| v.as_array())
            .expect("args should be an array");
        let arg_strs: Vec<&str> = args.iter().filter_map(|v| v.as_str()).collect();
        assert!(arg_strs.contains(&"--agent-id"));
        assert!(arg_strs.contains(&"claude-code"));
    }

    #[test]
    fn cli_resolution_mixes_sources() {
        // server from config, branch from flag, format default
        let mut config = CtxConfig::default();
        config.set_key("server", "http://config:3001").unwrap();

        let mut raw = raw_with_no_flags();
        raw.branch = Some("feature-x".to_string());

        let cli = Cli::from_raw(raw, &config);
        assert_eq!(cli.server, "http://config:3001");
        assert_eq!(cli.branch, "feature-x");
        assert_eq!(cli.format, OutputFormat::Text);
    }
}
