mod ingest;
mod metrics;
mod onboarding;
mod service;

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
const EX_TEMPFAIL: i32 = 75; // temporary failure (lock held, etc.)
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

    /// Namespace to operate in (env: CTX_NAMESPACE). When omitted, the
    /// project detection chain runs for the cwd (.ctxproject walk-up,
    /// then git remote lookup); no match → the "default" namespace.
    #[arg(long, env = "CTX_NAMESPACE", global = true)]
    namespace: Option<String>,

    /// Bearer token for a hub that requires auth (env: CTX_TOKEN). Sent as
    /// `Authorization: Bearer <token>`. Only needed for a non-loopback hub —
    /// loopback requests are exempt.
    #[arg(long, env = "CTX_TOKEN", global = true)]
    token: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

/// Fully-resolved CLI with defaults applied.
/// Priority: flag → env var → config file → hardcoded default.
struct Cli {
    server: String,
    branch: String,
    /// True when the branch came from --branch / CTX_BRANCH (not the
    /// config file). Explicit branches suppress git-branch mirroring.
    branch_explicit: bool,
    format: OutputFormat,
    session: Option<String>,
    /// Explicit namespace from --namespace / CTX_NAMESPACE. `None` means
    /// "detect from cwd" (see [`Cli::resolve_namespace`]).
    namespace: Option<String>,
    /// Bearer token for an authenticated hub (--token / CTX_TOKEN).
    token: Option<String>,
    command: Commands,
}

impl Cli {
    fn from_raw(raw: RawCli, config: &CtxConfig) -> Self {
        let branch_explicit = raw.branch.is_some();
        Self {
            server: raw
                .server
                .or_else(|| config.server.clone())
                .unwrap_or_else(|| "http://localhost:3001".to_string()),
            branch: raw
                .branch
                .or_else(|| config.branch.clone())
                .unwrap_or_else(|| "main".to_string()),
            branch_explicit,
            format: raw.format.or(config.format).unwrap_or(OutputFormat::Text),
            session: raw.session,
            namespace: raw.namespace,
            token: raw.token,
            command: raw.command,
        }
    }

    /// Resolve the namespace for this invocation. An explicit
    /// --namespace / CTX_NAMESPACE wins; otherwise ask the Hub to run
    /// the project detection chain for the cwd. Returns `None` (→ the
    /// "default" namespace, no header sent) when nothing matches or the
    /// Hub is unreachable — namespace resolution must never block a
    /// command, so failures here are silent by design.
    async fn resolve_namespace(&self) -> Option<String> {
        if let Some(ns) = &self.namespace {
            return Some(ns.clone());
        }
        let cwd = std::env::current_dir().ok()?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1500))
            .build()
            .ok()?;
        let resp = client
            .get(format!("{}/api/projects/detect", self.server))
            .query(&[("cwd", cwd.to_string_lossy().as_ref())])
            .send()
            .await
            .ok()?;
        let v: serde_json::Value = resp.json().await.ok()?;
        if v["status"] == "found" {
            v["namespace"].as_str().map(str::to_string)
        } else {
            None
        }
    }

    /// Build a reqwest client with X-CTXone-Session, X-CTXone-Namespace, and
    /// (for an authenticated hub) `Authorization: Bearer <token>` baked in as
    /// default headers.
    fn http_client(&self, namespace: Option<&str>) -> reqwest::Client {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(ref sid) = self.session
            && let Ok(val) = reqwest::header::HeaderValue::from_str(sid)
        {
            headers.insert("X-CTXone-Session", val);
        }
        if let Some(ns) = namespace
            && let Ok(val) = reqwest::header::HeaderValue::from_str(ns)
        {
            headers.insert("X-CTXone-Namespace", val);
        }
        if let Some(ref token) = self.token
            && let Ok(mut val) =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        {
            val.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        let builder = reqwest::Client::builder().default_headers(headers);
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
    /// Install CTXone's agent Skill (SKILL.md) into every detected agent skill
    /// dir so tools like Claude Code auto-load CTX usage guidance. Run once per
    /// machine (or `--project` to scope to this repo). `--status` reports what's
    /// installed, `--remove` uninstalls, `--dry-run` previews, `--emit-spec`
    /// prints the SkillSpec JSON for the combined suite.
    Skill {
        /// Install project-scoped (into the repo) instead of user-wide.
        #[arg(long)]
        project: bool,
        /// Only install for one host key (e.g. claude-code).
        #[arg(long)]
        tool: Option<String>,
        /// Remove installed skill files instead of writing them.
        #[arg(long)]
        remove: bool,
        /// Report install state without changing anything.
        #[arg(long)]
        status: bool,
        /// Suppress the one-time suggestion to add ASD (also CTX_NO_SUGGEST=1).
        #[arg(long)]
        no_nudge: bool,
        /// Print CTX's SkillSpec as JSON and exit (cross-CLI contract for the
        /// combined suite skill).
        #[arg(long)]
        emit_spec: bool,
        /// Print what would happen without touching the filesystem.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print a paste-into-your-agent block that installs + primes CTX (+ ASD).
    Bootstrap,
    /// Store a durable fact in agent memory so it survives across sessions and
    /// is retrievable via `recall`/`search`. Call this the moment you learn a
    /// decision, convention, gotcha, or preference worth not re-deriving later.
    Remember {
        /// The fact to remember. Pass "-" (or pipe with no value) to read the
        /// fact from stdin — handy for multi-line facts or `cmd | ctx remember -`.
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
    /// Retrieve the most relevant memories for a topic within a token budget —
    /// LLM-oriented (ranked + budgeted, pinned context always included). Call
    /// this at the start of a task instead of re-reading docs/files. For an
    /// exhaustive literal-substring scan with no budget, use `search` instead.
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
    /// Load the full stored context for a named project (everything under that
    /// project's context path), unranked and unbudgeted. Use when you want the
    /// whole picture for a project rather than a topic-ranked slice — for a
    /// budgeted, topic-scoped view use `recall`.
    Context {
        /// Project context key (the tag facts were stored under, not the
        /// namespace resolved by `ctx project`).
        project: String,
    },
    /// Show Hub status and connection info
    Status,
    /// Show cumulative token-savings statistics for this session (tokens used
    /// vs. saved, ratio). For live per-session Claude Code transcript analysis
    /// use `session metrics`; for hub/connection health use `status`.
    Stats,
    /// Start the CtxOne Hub server in the foreground. Good for dev/one-off runs;
    /// for an always-on daemon that owns the db across reboots use
    /// `ctx service install`. `--http` also serves the REST/MCP API; add
    /// `--lens` for the web UI (requires --http).
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
    /// Import a Claude Code session JSONL into CTXone memory
    ///
    /// Extracts structured memories and token usage from each conversation
    /// turn and stores them in the Hub. Requires ANTHROPIC_API_KEY.
    IngestSession {
        /// Path to a specific .jsonl file (default: all sessions for this project)
        #[arg(long)]
        file: Option<String>,

        /// Scan EVERY project under ~/.claude/projects (not just this one).
        /// Forces full-turn + token capture; extraction still requires an API
        /// key. Prints per-project counts and a final JSON summary line.
        #[arg(long)]
        all: bool,

        /// Only import sessions modified after this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// Process only the last N turns (default: all)
        #[arg(long)]
        last: Option<usize>,

        /// Skip memory extraction, only record token usage
        #[arg(long)]
        tokens_only: bool,

        /// Dry run: show what would be stored without writing
        #[arg(long)]
        dry_run: bool,

        /// Force persisting the full turn JSON (default anyway; explicit for
        /// callers that want the guarantee, e.g. the hub's session-sync).
        #[arg(long)]
        full_turn: bool,

        /// Skip persisting the full turn JSON (only extracted memories + tokens)
        #[arg(long, conflicts_with = "full_turn")]
        no_full_turn: bool,
    },

    /// Analyze token usage, cost, and cache metrics for Claude Code sessions.
    ///
    /// Parses JSONL transcripts from ~/.claude/projects/ and produces a
    /// summary of token spend, cache hit rates, cost estimates (with vs.
    /// without caching), and unit-of-work breakdown by time gap.
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Capture memories and token usage from the last agent turn.
    ///
    /// Called automatically by the Claude Code / Codex Stop hook.
    /// Reads hook payload JSON from stdin (contains transcript_path).
    /// Requires ANTHROPIC_API_KEY.
    CaptureTurn {
        /// Path to transcript file (overrides stdin payload)
        #[arg(long)]
        transcript: Option<String>,

        /// Session ID to attach to stored memories and token records
        #[arg(long, env = "CTX_SESSION")]
        session: Option<String>,

        /// Number of recent turns to process (default: 1)
        #[arg(long, default_value = "1")]
        turns: usize,

        /// Skip memory extraction, only record token usage
        #[arg(long)]
        tokens_only: bool,

        /// Skip persisting the full turn JSON (only extracted memories + tokens)
        #[arg(long)]
        no_full_turn: bool,
    },

    /// Seed the Hub with realistic demo data and show live token savings
    Demo,
    /// List pinned memories (always-included critical context)
    Pinned,
    /// Import a markdown doc into memory (alias: `import-doc`). Parses the file
    /// into sections stored as searchable memories so agents can `recall` it
    /// without re-reading the file. `--pin` makes every section always-included
    /// in recall (critical context). This is the intended "register a doc as
    /// memory" flow — keep the file canonical in the repo and import its
    /// rationale/summary here rather than pasting the whole doc into memory.
    #[command(visible_alias = "import-doc")]
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
    /// Trace decision provenance — find facts/decisions that mention a
    /// phrase and return the blame chain (who wrote each, when, why) for
    /// every match. Use this before reversing or debating a settled
    /// decision, especially security/licensing/deployment choices.
    /// Mirrors the `why_did_we` MCP tool.
    WhyDidWe {
        /// Decision phrase to search for, e.g. "BSL", "SQLite", "Postgres".
        decision: String,
    },
    /// Capture key points + decisions from the current (or named) session.
    /// Stored under /sessions/<id>/{summary,decisions}. Mirrors the
    /// `summarize_session` MCP tool. Session id resolves from the global
    /// `--session` flag or the `CTX_SESSION` env.
    SummarizeSession {
        /// One bullet at a time; pass --point multiple times.
        #[arg(long = "point", short = 'p', required = true)]
        points: Vec<String>,
        /// Decision recorded for the session; pass --decision multiple times.
        #[arg(long = "decision", short = 'd')]
        decisions: Vec<String>,
    },
    /// Report LLM token usage for the current session — accumulates per
    /// session counters in the Hub. Mirrors `record_llm_usage` MCP.
    RecordUsage {
        /// Input/prompt tokens consumed.
        #[arg(long = "input")]
        input_tokens: u64,
        /// Output/completion tokens generated.
        #[arg(long = "output")]
        output_tokens: u64,
        /// Cache-hit (read) tokens (default 0).
        #[arg(long = "cache-read", default_value_t = 0)]
        cache_read_tokens: u64,
        /// Cache-creation tokens (default 0).
        #[arg(long = "cache-create", default_value_t = 0)]
        cache_create_tokens: u64,
        /// Model id, e.g. "claude-sonnet-4-5".
        #[arg(long)]
        model: Option<String>,
        /// Provider id, e.g. "anthropic".
        #[arg(long)]
        provider: Option<String>,
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
        /// MCP transport to configure. `http` (default, recommended) points
        /// the tool at a shared daemon's `/mcp` URL — run one
        /// `ctxone-hub --http --lens` (or `ctx service install`, see
        /// docs/DEPLOYMENT.md) so a single process serves MCP + REST + Lens
        /// with no lockfile races. `stdio` is the escape hatch: each tool
        /// spawns its own `ctxone-hub` child that owns the db (zero-setup, but
        /// only one owner per db and no shared web UI).
        #[arg(long, value_enum, default_value_t = McpTransport::Http)]
        transport: McpTransport,
        /// Base URL of the shared hub's MCP endpoint (only used with
        /// `--transport http`). The project namespace is appended as
        /// `?namespace=<ns>` when one is detected.
        #[arg(long, default_value = "http://localhost:3001/mcp")]
        mcp_url: String,
        /// Literal bearer token to embed in generated http configs (for a
        /// hub started with --auth-token). WARNING: written in plaintext into
        /// the tool's config file. Used for native `headers` (Claude Code,
        /// Cursor, VS Code) and the mcp-remote `--header` (Claude Desktop).
        /// Codex needs an env-var name — see `--auth-token-env`.
        #[arg(long)]
        auth_token: Option<String>,
        /// Name of an environment variable the tool reads the bearer token
        /// from at runtime (keeps the secret out of the config file). This is
        /// how Codex takes a token (`bearer_token_env_var`). Prefer this over
        /// `--auth-token` where the client supports it.
        #[arg(long)]
        auth_token_env: Option<String>,
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
    /// Manage taints — markers that flag paths as needing verification,
    /// blocking write attempts, or watching for changes. Three kinds:
    /// `taint` (effect-based), `quarantine` (per-agent gate), `watch`
    /// (observe-only). Mirrors the `taint_*` MCP tools.
    Taint {
        #[command(subcommand)]
        action: TaintAction,
    },
    /// Database admin. Two axes:
    ///   • Whole-db files: `backup` (live VACUUM INTO snapshot, safe against a
    ///     running hub) and `restore` (file-level swap; hub MUST be stopped).
    ///   • Portable content: `export` a branch to JSON and `import` it back —
    ///     to prune, share, or seed a fresh db (branch/namespace aware).
    /// Use backup/restore for disaster recovery; export/import to move or trim
    /// graph content.
    Db {
        #[command(subcommand)]
        action: DbAction,
    },
    /// Doc registry — index your canonical `.md` docs (path, status, scope,
    /// what they answer) so agents can find the right doc without scanning the
    /// repo. Keep the file canonical in the repo; this stores the pointer.
    Docs {
        #[command(subcommand)]
        action: DocsAction,
    },
    /// Manage projects — a project maps a code repo to its own namespace
    /// holding that repo's branches, plans, memory, and ASD data. Detection
    /// is automatic per-command (`.ctxproject` file, then git remote); use
    /// `ctx project add` once per repo to opt in.
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Install the Hub as a login/boot service (launchd on macOS, systemd
    /// user unit on Linux) so the unified daemon (MCP + REST + Lens) owns
    /// the db before any agent starts — the fix for the reboot race.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Work with reminders — durable, scheduled follow-ups stored in the Hub.
    Reminder {
        #[command(subcommand)]
        action: ReminderAction,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceAction {
    /// Write and register the service unit. Runs `ctxone-hub --http --lens`.
    Install {
        /// HTTP port for the daemon.
        #[arg(long, default_value_t = 3001)]
        port: u16,
        /// SQLite db path the daemon owns. Defaults to the canonical path.
        #[arg(long)]
        path: Option<String>,
        /// Do not serve the Lens web UI (REST + MCP only).
        #[arg(long)]
        no_lens: bool,
        /// Embed a bearer token in the unit's environment (chmod 600). Prefer
        /// setting CTXONE_AUTH_TOKEN in your own environment for secrets.
        #[arg(long)]
        auth_token: Option<String>,
        /// Print the unit file and the commands without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite an existing unit file.
        #[arg(long)]
        force: bool,
    },
    /// Stop and remove the service.
    Uninstall {
        /// Print the commands without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the service's registration status.
    Status,
    /// Manage the periodic reminder-tick timer (runs `ctx reminder tick`).
    Tick {
        #[command(subcommand)]
        action: TickAction,
    },
}

#[derive(Subcommand, Debug)]
enum TickAction {
    /// Install the periodic timer that runs `ctx reminder tick`.
    Install {
        /// Seconds between ticks (default hourly).
        #[arg(long, default_value_t = 3600)]
        interval: u64,
        /// Allowlist path passed through to `ctx reminder tick`.
        #[arg(long, default_value = "~/.ctxone/reminder-tick.allow")]
        allowlist: String,
        /// Reminder id to skip (repeatable) — e.g. one with a dedicated runner.
        #[arg(long = "skip")]
        skip: Vec<String>,
        /// Print the unit(s) + commands without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite existing unit file(s).
        #[arg(long)]
        force: bool,
    },
    /// Stop and remove the tick timer.
    Uninstall {
        /// Print the commands without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show tick-timer registration status.
    Status,
}

#[derive(Subcommand, Debug)]
enum ProjectAction {
    /// Register the current repo (or --path) as a project. Creates the
    /// namespace on the Hub, binds the local path, records the git remote
    /// for detection, and writes a .ctxproject marker at the repo root.
    Add {
        /// Project id (kebab-case). Doubles as the namespace name
        /// unless --namespace is given.
        id: String,
        /// Human-readable name (defaults to the id).
        #[arg(long)]
        display_name: Option<String>,
        /// Repo root to bind (default: git root of cwd, else cwd).
        #[arg(long)]
        path: Option<String>,
        /// Skip writing the .ctxproject marker file.
        #[arg(long)]
        no_marker: bool,
    },
    /// List registered projects.
    List,
    /// Point this checkout at an existing project: writes .ctxproject at
    /// the repo root and binds the path on the Hub.
    Use {
        /// Project id to bind this checkout to.
        id: String,
        /// Skip writing the .ctxproject marker file.
        #[arg(long)]
        no_marker: bool,
    },
    /// Show which project/namespace the current directory resolves to.
    Detect,
}

#[derive(Subcommand, Debug)]
enum SessionAction {
    /// Show token usage metrics for Claude Code sessions in this project.
    Metrics {
        /// Project directory to analyze (default: current directory)
        #[arg(long)]
        project: Option<String>,
        /// Show a specific session by its UUID
        #[arg(long)]
        session: Option<String>,
        /// List sessions without full metrics detail
        #[arg(long)]
        list: bool,
        /// Analyze all projects in ~/.claude/projects/
        #[arg(long)]
        all: bool,
        /// Output as JSON (machine-readable)
        #[arg(long)]
        json: bool,
        /// Time-gap in minutes to split units of work (default: 5)
        #[arg(long, default_value_t = 5.0)]
        gap: f64,
        /// Show per-turn detail table
        #[arg(long)]
        verbose: bool,
    },
}

#[derive(Subcommand, Debug)]
enum TaintAction {
    /// List taints, optionally filtered by path prefix, kind, or
    /// resolved-status.
    List {
        /// Only taints whose path starts with this prefix.
        #[arg(long)]
        path_prefix: Option<String>,
        /// Filter by kind: taint|quarantine|watch.
        #[arg(long)]
        kind: Option<String>,
        /// Include resolved (untainted) entries (default: false).
        #[arg(long)]
        include_resolved: bool,
    },
    /// Check whether a write would be allowed at a path for an agent
    /// at a given confidence. Read-only — does not modify state.
    Check {
        /// Graph path to test a hypothetical write against.
        path: String,
        /// Agent attempting the write (defaults to session agent).
        #[arg(long = "as")]
        agent_id: Option<String>,
        /// Confidence of the proposed write (default 1.0).
        #[arg(long, default_value_t = 1.0)]
        confidence: f64,
    },
    /// Apply a taint to a path.
    Apply {
        /// Graph path to taint (prefix-matched by later write checks).
        path: String,
        /// Human-readable name for this taint.
        #[arg(long)]
        name: String,
        /// Kind: taint|quarantine|watch (default taint).
        #[arg(long, default_value = "taint")]
        kind: String,
        /// Effect (taint kind only): warn|block|review|isolate|advisory.
        /// Required for kind=taint, ignored otherwise.
        #[arg(long)]
        effect: Option<String>,
        /// Severity: low|medium|high|critical (default medium).
        #[arg(long, default_value = "medium")]
        severity: String,
        /// Why this is being tainted (recorded for audit).
        #[arg(long, short)]
        reason: String,
        /// For kind=quarantine: comma-separated agent ids allowed
        /// to write through the quarantine.
        #[arg(long, value_delimiter = ',')]
        authorized: Vec<String>,
    },
    /// Remove (resolve) a taint by id. Use `ctx taint list` to find ids.
    Remove {
        /// Taint id to resolve. Find it via `ctx taint list`.
        taint_id: String,
        /// Why the taint is being resolved (recorded for audit).
        #[arg(long, short)]
        reason: String,
    },
}

#[derive(Subcommand, Debug)]
enum DbAction {
    /// Trigger a snapshot of the hub's live db. The hub responds
    /// with the path it wrote (under <db>.bak.<utc>). Cheap — runs
    /// against a live hub via SQLite VACUUM INTO.
    Backup {
        /// Optional suffix override. Default: current UTC timestamp.
        #[arg(long)]
        suffix: Option<String>,
    },
    /// Restore the live db from a snapshot file. The hub MUST be
    /// stopped first — this command checks for an active lockfile
    /// (<db>.lock) and refuses if a hub is running. Renames the
    /// current db to `<db>.pre-restore-<utc>` so the operation is
    /// reversible.
    Restore {
        /// Path to the snapshot file (the .bak.* you want to restore).
        snapshot: String,
        /// Path to the live db file to overwrite. Must match the
        /// --path the hub will use on next start.
        #[arg(long)]
        to: String,
        /// Skip the y/N confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Export the graph on the current branch to a JSON snapshot (stdout or
    /// --out FILE). Prune it, then `db import` into a fresh db to keep only
    /// what you want. Use `ctx --branch <b>` / `--namespace <n>` to pick scope.
    Export {
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<String>,
    },
    /// Import a JSON snapshot (from `db export`) onto the current branch,
    /// writing each path into the graph. This MERGES (upserts): paths in the
    /// snapshot overwrite, paths already present but not in the snapshot are
    /// left untouched (internal `/_meta/*` is skipped). Target scope with
    /// `ctx --branch <b>` / `--namespace <n>`. Pair with `db export` → prune
    /// the JSON → import into a fresh db to keep only what you want.
    Import {
        /// Path to the snapshot JSON file.
        file: String,
    },
}

#[derive(Subcommand)]
enum DocsAction {
    /// Register (or update) a canonical doc. Re-adding the same path updates it.
    Add {
        /// Repo path of the doc, e.g. /ARCHITECTURE.md
        path: String,
        /// canonical | draft | superseded (default: canonical)
        #[arg(long)]
        status: Option<String>,
        /// Scope/owner area, e.g. "synth architecture"
        #[arg(long)]
        scope: Option<String>,
        /// Owner (person/team)
        #[arg(long)]
        owner: Option<String>,
        /// What questions this doc answers
        #[arg(long)]
        answers: Option<String>,
        /// Path of a doc this supersedes
        #[arg(long)]
        supersedes: Option<String>,
        /// Commit at which the doc was last verified current
        #[arg(long = "verified-commit")]
        verified_commit: Option<String>,
    },
    /// List all registered docs.
    List,
    /// Find registered docs whose path/scope/answers match a query.
    Find { query: String },
}

#[derive(Subcommand)]
enum ReminderAction {
    /// Run due, approved reminders whose commands are all allowlisted, then
    /// record the outcome. Meant to run on a timer (see `ctx service`).
    ///
    /// A reminder runs only if BOTH gates pass: (1) it is explicitly
    /// `autonomous: true` (anything else — including `awaiting_permission` —
    /// needs a human); and (2) every one of its `commands` is on the allowlist
    /// (one exact command per line, '#' comments). Otherwise nothing from it
    /// runs and it is recorded `deferred` and snoozed. Missing allowlist =
    /// nothing runs.
    Tick {
        /// Allowlist file of approved commands (exact match, one per line).
        #[arg(long, default_value = "~/.ctxone/reminder-tick.allow")]
        allowlist: String,
        /// Reminder id to skip (repeatable) — e.g. one with a dedicated runner.
        #[arg(long = "skip")]
        skip: Vec<String>,
        /// Hours to snooze a reminder whose commands aren't allowlisted.
        #[arg(long, default_value_t = 12)]
        defer_hours: i64,
        /// Show what would run/defer without executing or recording.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Per-kind proof examples, shown in error messages to speed recovery.
const PROOF_EXAMPLES: &str =
    "commit:abc1234 | test:cargo test foo | file:src/lib.rs | text:manually verified in staging";

/// Proof specifier — `kind:value[:note]`. Kind is one of commit|file|test|text.
fn parse_proof_spec(raw: &str) -> Result<(String, String, Option<String>), String> {
    let parts: Vec<&str> = raw.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(format!(
            "invalid proof '{raw}': expected <kind>:<value>[:<note>]. Examples: {PROOF_EXAMPLES}"
        ));
    }
    let kind = parts[0].to_string();
    let value = parts[1].to_string();
    let note = parts.get(2).map(|s| s.to_string());
    match kind.as_str() {
        "commit" | "file" | "test" | "text" => Ok((kind, value, note)),
        _ => Err(format!(
            "unknown proof kind '{kind}' (expected commit|file|test|text). Examples: {PROOF_EXAMPLES}"
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
        /// Bypass the "plan nearing completion" lock (Hub env var
        /// `CTXONE_PLAN_LOCK_RATIO`). Has no effect unless the Hub
        /// has the lock enabled.
        #[arg(long)]
        force: bool,
    },
    /// Mark a task in-progress before you begin work, so `plan next` skips it
    /// and `plan stale` can track it. Warns (non-blocking) if another task in
    /// the plan is already in-progress.
    Start {
        /// Plan the task belongs to.
        plan_id: String,
        /// Task id to start (e.g. t-003).
        task_id: String,
        /// Optional note recorded in blame (e.g. why you picked this now).
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
        /// Pick the first unstarted task by task order (sequential plans)
        /// instead of the default highest-priority task.
        #[arg(long = "in-order")]
        in_order: bool,
    },
    /// List plans
    List {
        /// Filter by status: active|completed|archived
        #[arg(long)]
        status: Option<String>,
        /// List plans across every namespace (each shown with its namespace),
        /// instead of only the current one.
        #[arg(long = "all-namespaces")]
        all_namespaces: bool,
    },
    /// Record that a task, when done, satisfies a task in another plan
    Link {
        /// Plan holding the task that does the satisfying
        plan_id: String,
        /// Task id in that plan (e.g. t-003)
        task_id: String,
        /// Target it satisfies, as `plan/task` (e.g. other-plan/t-002)
        target: String,
    },
    /// List in-progress tasks that have gone stale (no progress in N days)
    Stale {
        /// Consider a task stale after this many days in progress
        #[arg(long, default_value_t = 7)]
        days: i64,
        /// Scan every namespace, not just the current one
        #[arg(long = "all-namespaces")]
        all_namespaces: bool,
    },
    /// Show a plan with its tasks
    Show { plan_id: String },
    /// List the tasks of a plan (flat — no plan envelope). Use `show`
    /// when you also want plan metadata.
    Tasks { plan_id: String },
    /// Archive a plan (soft — task data preserved)
    Archive { plan_id: String },
    /// Force-complete a plan: abandon every still-open task with a fixed
    /// reason and let the engine promote the plan to `completed`.
    /// Idempotent on already-completed plans; rejected on archived or
    /// empty plans.
    Complete {
        plan_id: String,
        /// Reason recorded on every abandoned task. Defaults to
        /// "Plan force-completed by user".
        #[arg(long, short)]
        reason: Option<String>,
    },
    /// Move a plan from one branch to another. The source branch is
    /// the active --branch flag (or "main"); the target is required.
    /// Task ids and statuses are preserved.
    Move {
        plan_id: String,
        /// Branch to move the plan onto (must differ from --branch).
        #[arg(long = "to")]
        target: String,
    },
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
    // Resolve the namespace once per invocation, but only for commands
    // that talk to the Hub — purely-local commands (and `serve`, where
    // the Hub is by definition not up yet) skip the detection round-trip.
    let namespace = match &cli.command {
        // `init` needs the project namespace: `--transport http` bakes it into
        // the `/mcp?namespace=<ns>` URL, and both transports use it as the
        // stable CTX_SESSION id injected into the stdio server's env (t-015).
        // Best-effort — a down hub just yields None and we fall back below.
        Commands::Init { .. } => cli.resolve_namespace().await,
        Commands::Skill { .. }
        | Commands::Bootstrap
        | Commands::Completion { .. }
        | Commands::Config { .. }
        | Commands::Serve { .. }
        | Commands::Session { .. }
        | Commands::Service { .. }
        | Commands::Db { .. } => None,
        _ => cli.resolve_namespace().await,
    };
    let client = cli.http_client(namespace.as_deref());

    // Branch mirroring: inside a project namespace, default the working
    // branch to the sanitized current git branch. Explicit --branch /
    // CTX_BRANCH wins; the config-file default applies outside projects.
    // The ensure call is idempotent and fails silently — if the Hub is
    // down, the actual command will report it properly.
    let mut cli = cli;
    if namespace.is_some()
        && !cli.branch_explicit
        && let Ok(cwd) = std::env::current_dir()
        && let Some(raw_branch) = read_git_branch(&cwd)
    {
        let mirrored = sanitize_branch_name(&raw_branch);
        cli.branch = mirrored.clone();
        if mirrored != "main" {
            let _ = client
                .post(format!("{}/api/branches", cli.server))
                .json(&serde_json::json!({
                    "name": mirrored,
                    "from": "main",
                    "if_missing": true,
                    "git_branch": raw_branch,
                }))
                .send()
                .await;
        }
    }
    let cli = cli;

    match cli.command {
        Commands::Skill {
            project,
            tool,
            remove,
            status,
            no_nudge,
            emit_spec,
            dry_run,
        } => {
            onboarding::run_skill(
                project,
                tool.as_deref(),
                remove,
                status,
                no_nudge,
                emit_spec,
                dry_run,
            )?;
        }
        Commands::Bootstrap => {
            onboarding::run_bootstrap();
        }
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

            let resp = match client
                .clone()
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
            let resp = match client.get(&url).send().await {
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
                let exact_flat = match client.get(&flat_url).send().await {
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
            let resp = match client.get(&url).send().await {
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
            let reachable = client.get(&health_url).send()
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
                "namespace": namespace.as_deref().unwrap_or("default"),
            });
            if let Some(ns) = &namespace
                && let Ok(r) = client.get(format!("{}/api/projects/detect?cwd={}",
                    cli.server,
                    std::env::current_dir()
                        .map(|d| d.to_string_lossy().into_owned())
                        .unwrap_or_default())).send()
                    .await
                && let Ok(det) = r.json::<Value>().await
                && det["status"] == "found"
                && det["namespace"].as_str() == Some(ns.as_str())
            {
                out["project"] = det["project_id"].clone();
                out["project_via"] = det["via"].clone();
            }
            if let Ok(r) = client.get(format!("{}/api/stats/tokens", cli.server)).send().await
                && let Ok(parsed) = r.json::<Value>().await
            {
                out["tokens"] = parsed;
            }
            emit(cli.format, &out, |v| {
                println!("Hub: connected ({})", cli.server);
                match v.get("project").and_then(|p| p.as_str()) {
                    Some(p) => println!(
                        "Project: {} (namespace: {}, via {})",
                        p,
                        v["namespace"].as_str().unwrap_or("default"),
                        v["project_via"].as_str().unwrap_or("?"),
                    ),
                    None => println!(
                        "Namespace: {}",
                        v["namespace"].as_str().unwrap_or("default")
                    ),
                }
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
            let resp = match client.get(format!("{}/api/stats/tokens", cli.server)).send().await {
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

        Commands::Session { action } => {
            run_session_action(action).await?;
        }

        Commands::IngestSession {
            file,
            all,
            since,
            last,
            tokens_only,
            dry_run,
            full_turn,
            no_full_turn,
        } => {
            // `--full-turn` forces on; `--no-full-turn` forces off; default is on.
            let full_turn_effective = full_turn || !no_full_turn;
            run_ingest_session(
                &cli.server,
                &cli.branch,
                cli.session.as_deref(),
                file,
                all,
                since,
                last,
                tokens_only,
                dry_run,
                full_turn_effective,
                client.clone(),
            )
            .await?;
        }

        Commands::CaptureTurn {
            transcript,
            session,
            turns,
            tokens_only,
            no_full_turn,
        } => {
            // Prefer explicit --session flag, then CLI global, then CTX_SESSION env.
            let sid = session
                .or(cli.session.clone())
                .or_else(|| std::env::var("CTX_SESSION").ok());
            run_capture_turn(
                &cli.server,
                &cli.branch,
                sid.as_deref(),
                transcript,
                turns,
                tokens_only,
                !no_full_turn,
                client.clone(),
            )
            .await?;
        }
        Commands::Pinned => {
            let resp = match client.get(format!("{}/api/memory/pinned", cli.server)).send().await {
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

            let resp = match client
                .clone()
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
            let resp = match client.get(&url).send().await {
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

            let resp = match client
                .clone()
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

            let resp = match client
                .clone()
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
        Commands::WhyDidWe { decision } => {
            // Mirrors the `why_did_we` MCP tool: search_values + blame
            // for every matching path. The HTTP route always reads from
            // `main` (not the per-request branch) — that's intentional
            // because decision provenance is global, not branch-scoped.
            let url = format!(
                "{}/api/memory/why_did_we?decision={}",
                cli.server,
                urlencoding(&decision),
            );
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "why-did-we failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let traces = v["traces"].as_array().cloned().unwrap_or_default();
                if traces.is_empty() {
                    println!("No traced decisions for '{}'", decision);
                    return;
                }
                println!(
                    "Found {} trace{} for '{}':",
                    traces.len(),
                    if traces.len() == 1 { "" } else { "s" },
                    decision
                );
                for t in &traces {
                    let path = t["path"].as_str().unwrap_or("");
                    let blame = &t["blame"];
                    println!();
                    println!("  {}", path);
                    let by = blame["agent_id"].as_str().unwrap_or("?");
                    let at = blame["timestamp"].as_str().unwrap_or("?");
                    let why = blame["intent_description"].as_str().unwrap_or("");
                    let category = blame["intent_category"].as_str().unwrap_or("");
                    println!("    {} @ {} [{}]", by, at, category);
                    if !why.is_empty() {
                        println!("    {}", why);
                    }
                }
            });
        }
        Commands::SummarizeSession { points, decisions } => {
            // Mirrors the summarize_session MCP tool. Session id comes
            // from the global --session flag (or CTX_SESSION env); when
            // absent we error so the capture is always anchored.
            let session_id = cli.session.clone().ok_or_else(|| {
                "no session id (pass --session <id> or set CTX_SESSION)".to_string()
            })?;
            let body = serde_json::json!({
                "session_id": session_id,
                "key_points": points,
                "decisions": decisions,
            });
            let url = format!("{}/api/memory/summarize_session", cli.server);
            let resp = match client.post(&url).json(&body).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "summarize-session failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                let kp = v["key_points"].as_u64().unwrap_or(0);
                let de = v["decisions"].as_u64().unwrap_or(0);
                println!(
                    "Captured {} key point{}, {} decision{} for session {}",
                    kp,
                    if kp == 1 { "" } else { "s" },
                    de,
                    if de == 1 { "" } else { "s" },
                    session_id
                );
            });
        }
        Commands::RecordUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_create_tokens,
            model,
            provider,
        } => {
            let mut body = serde_json::json!({
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_read_tokens": cache_read_tokens,
                "cache_create_tokens": cache_create_tokens,
            });
            if let Some(m) = model {
                body["model"] = serde_json::json!(m);
            }
            if let Some(p) = provider {
                body["provider"] = serde_json::json!(p);
            }
            let url = format!("{}/api/stats/llm_usage", cli.server);
            let resp = match client.post(&url).json(&body).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(&cli.server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "record-usage failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(cli.format, &parsed, |v| {
                // SessionSnapshot field names — be defensive across
                // schema drift.
                let total = v["total_tokens"]
                    .as_u64()
                    .or_else(|| v["llm_total_tokens"].as_u64())
                    .unwrap_or(0);
                let session = v["session_id"].as_str().unwrap_or("?");
                println!(
                    "Recorded {} in / {} out (session totals: {} tokens) for session {}",
                    input_tokens, output_tokens, total, session
                );
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
            let resp = match client.get(&url).send().await {
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
            let resp = match client.get(&url).send().await {
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
            let resp = match client.get(&url).send().await {
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
            let resp = match client.get(&url).send().await {
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
            let resp = match client.get(&url).send().await {
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
            run_tail(&cli.server, &cli.branch, interval, client.clone()).await?;
        }
        Commands::Branches => {
            let resp = match client.get(format!("{}/api/branches", cli.server)).send().await {
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
            let resp = match client
                .clone()
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
            transport,
            mcp_url,
            auth_token,
            auth_token_env,
        } => {
            // Grab the fields agents_install_prompt needs BEFORE the
            // match consumes `cli.command` via destructuring. We only
            // need server + branch + format for the Agents handlers,
            // and the other arms don't touch them.
            let server = cli.server.clone();
            let branch = cli.branch.clone();
            let format = cli.format;
            // Preflight: for http configs, check the daemon is actually up so we
            // don't hand tools a URL config pointing at a hub that isn't running.
            // Skipped in --dry-run (nothing is written anyway).
            if transport == McpTransport::Http
                && !dry_run
                && let Some(health) = hub_health_url(&mcp_url)
            {
                let reachable = client
                    .get(&health)
                    .timeout(std::time::Duration::from_millis(1500))
                    .send()
                    .await
                    .map(|r| r.status().is_success())
                    .unwrap_or(false);
                if !reachable {
                    eprintln!(
                        "  \u{26A0} hub not reachable at {health} — the configs below will \
                         point at a hub that isn't running yet. Start it with \
                         `ctxone-hub --http --lens` or `ctx service install`."
                    );
                }
            }
            // `namespace` (resolved above for --transport http) is baked into
            // the `/mcp?namespace=<ns>` URL so the shared daemon scopes writes
            // the way a per-project stdio hub would.
            init_mcp(
                global,
                tool,
                config_path,
                dry_run,
                transport,
                &mcp_url,
                namespace.clone(),
                auth_token.as_deref(),
                auth_token_env.as_deref(),
            )?;
            // After MCP configs are written, optionally prime the
            // AGENTS.md guidance into the Hub. Skipped in --dry-run
            // (we don't want a dry run to actually write to the
            // graph) and when the user passed --no-agents.
            if !dry_run && !no_agents {
                println!();
                if let Err(e) =
                    agents_install_prompt(&server, &branch, format, client.clone()).await
                {
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
        Commands::Taint { action } => {
            let server = cli.server.clone();
            let branch = cli.branch.clone();
            let format = cli.format;
            handle_taint(action, &server, &branch, format, client.clone()).await?;
        }
        Commands::Db { action } => {
            let server = cli.server.clone();
            let branch = cli.branch.clone();
            let format = cli.format;
            handle_db(action, &server, &branch, format, client.clone()).await?;
        }
        Commands::Docs { action } => {
            let server = cli.server.clone();
            let branch = cli.branch.clone();
            let format = cli.format;
            handle_docs(action, &server, &branch, format, client.clone()).await?;
        }
        Commands::Project { action } => {
            let server = cli.server.clone();
            let format = cli.format;
            handle_project(action, &server, format, client.clone()).await?;
        }
        Commands::Service { action } => {
            handle_service(action)?;
        }
        Commands::Reminder { action } => {
            let server = cli.server.clone();
            let format = cli.format;
            handle_reminder(action, &server, format, client.clone()).await?;
        }
    }

    Ok(())
}

/// `ctx reminder` dispatch — the tick executes due, approved, allowlisted
/// reminders and records the outcome.
async fn handle_reminder(
    action: ReminderAction,
    server: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;
    match action {
        ReminderAction::Tick {
            allowlist,
            skip,
            defer_hours,
            dry_run,
        } => {
            let allow = load_tick_allowlist(&allowlist);
            let skip: std::collections::HashSet<String> = skip.into_iter().collect();

            let resp = match client
                .get(format!("{server}/api/reminders/due"))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "reminder tick: fetch due failed").await;
            }
            let due: Value = resp.json().await?;
            let due = due.as_array().cloned().unwrap_or_default();

            let (mut acted, mut deferred, mut skipped, mut needs_approval, mut no_cmds) =
                (0u32, 0u32, 0u32, 0u32, 0u32);
            let mut results: Vec<Value> = Vec::new();

            for r in &due {
                let id = r["id"].as_str().unwrap_or("").to_string();
                let title = r["title"].as_str().unwrap_or("?").to_string();
                let status = r["status"].as_str().unwrap_or("");
                let autonomous = r["autonomous"].as_bool().unwrap_or(false);

                // Gate 1: approval — fail closed. Only run reminders explicitly
                // marked `autonomous: true`; everything else (including
                // `awaiting_permission`) needs a human. Gating on the flag
                // directly, not just status, sidesteps an upstream bug where the
                // hub can promote a non-autonomous reminder straight to `due`
                // without routing it through `awaiting_permission`.
                if !autonomous || status == "awaiting_permission" {
                    needs_approval += 1;
                    continue;
                }
                if id.is_empty() || skip.contains(&id) {
                    skipped += 1;
                    continue;
                }

                let cmds: Vec<String> = r["commands"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty() && !s.starts_with('#'))
                            .collect()
                    })
                    .unwrap_or_default();
                if cmds.is_empty() {
                    no_cmds += 1;
                    continue;
                }

                // Gate 2: allowlist. Every command must be pre-approved.
                let disallowed: Vec<String> =
                    cmds.iter().filter(|c| !allow.contains(*c)).cloned().collect();
                if !disallowed.is_empty() {
                    deferred += 1;
                    let note = format!(
                        "not in allowlist — needs approval: {}",
                        disallowed.join(" ; ")
                    );
                    results.push(json!({"reminder": title, "action": "deferred", "detail": note}));
                    if !dry_run {
                        let _ =
                            reminder_record(&client, server, &id, "deferred", vec![note]).await;
                        let until = (chrono::Utc::now()
                            + chrono::Duration::hours(defer_hours))
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                        let _ = client
                            .post(format!("{server}/api/reminders/{id}/snooze"))
                            .json(&json!({"id": id, "until": until}))
                            .send()
                            .await;
                    }
                    continue;
                }

                if dry_run {
                    acted += 1;
                    results.push(json!({"reminder": title, "action": "would-run", "commands": cmds}));
                    continue;
                }

                // Both gates passed — run the commands.
                let _ = client
                    .post(format!("{server}/api/reminders/{id}/start"))
                    .json(&json!({}))
                    .send()
                    .await;
                let mut ok = true;
                let mut outs: Vec<String> = Vec::new();
                for c in &cmds {
                    match std::process::Command::new("bash").arg("-lc").arg(c).output() {
                        Ok(out) => {
                            let code = out.status.code().unwrap_or(-1);
                            let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
                            combined.push_str(&String::from_utf8_lossy(&out.stderr));
                            let last: String = combined
                                .lines()
                                .last()
                                .unwrap_or("")
                                .chars()
                                .take(120)
                                .collect();
                            outs.push(format!("$ {c} -> exit {code}: {last}"));
                            if !out.status.success() {
                                ok = false;
                            }
                        }
                        Err(e) => {
                            outs.push(format!("$ {c} -> spawn error: {e}"));
                            ok = false;
                        }
                    }
                }
                let result = if ok { "success" } else { "failed" };
                let _ = reminder_record(&client, server, &id, result, outs.clone()).await;
                acted += 1;
                results.push(json!({"reminder": title, "action": result, "output": outs}));
            }

            let summary = json!({
                "due": due.len(),
                "acted": acted,
                "deferred": deferred,
                "skipped": skipped,
                "needs_approval": needs_approval,
                "no_commands": no_cmds,
                "dry_run": dry_run,
                "results": results,
            });
            emit(format, &summary, |_| {
                println!(
                    "reminder tick: due={} acted={} deferred={} skipped={} awaiting_approval={} no_commands={}{}",
                    due.len(),
                    acted,
                    deferred,
                    skipped,
                    needs_approval,
                    no_cmds,
                    if dry_run { " (dry-run)" } else { "" }
                );
                for r in &results {
                    println!(
                        "  {} — {}",
                        r["reminder"].as_str().unwrap_or("?"),
                        r["action"].as_str().unwrap_or("?")
                    );
                }
            });
        }
    }
    Ok(())
}

/// POST a reminder execution record (best-effort helper for the tick).
async fn reminder_record(
    client: &reqwest::Client,
    server: &str,
    id: &str,
    result: &str,
    notes: Vec<String>,
) -> Result<(), reqwest::Error> {
    client
        .post(format!("{server}/api/reminders/{id}/record"))
        .json(&serde_json::json!({"id": id, "result": result, "notes": notes}))
        .send()
        .await
        .map(|_| ())
}

/// Load the tick allowlist: one exact command per line, '#' comments and blank
/// lines ignored. Missing file -> empty set (fail closed). Expands a leading `~/`.
fn load_tick_allowlist(path: &str) -> std::collections::HashSet<String> {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(rest))
            .unwrap_or_else(|| std::path::PathBuf::from(path)),
        None => std::path::PathBuf::from(path),
    };
    std::fs::read_to_string(&expanded)
        .map(|s| {
            s.lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `ctx service` dispatch — installs/removes the Hub as a login/boot service.
fn handle_service(action: ServiceAction) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        ServiceAction::Install {
            port,
            path,
            no_lens,
            auth_token,
            dry_run,
            force,
        } => {
            let spec = service::ServiceSpec {
                hub_bin: find_hub_binary(),
                db_path: path.unwrap_or_else(canonical_db_path),
                port,
                lens: !no_lens,
                auth_token,
                log_path: service::default_log_path(),
            };
            service::install(&spec, dry_run, force)
        }
        ServiceAction::Uninstall { dry_run } => service::uninstall(dry_run),
        ServiceAction::Status => service::status(),
        ServiceAction::Tick { action } => match action {
            TickAction::Install {
                interval,
                allowlist,
                skip,
                dry_run,
                force,
            } => {
                let spec = service::TickSpec {
                    ctx_bin: current_ctx_bin(),
                    interval_secs: interval,
                    allowlist: Some(allowlist),
                    skip,
                    server: None,
                    log_path: service::tick_log_path(),
                };
                service::tick_install(&spec, dry_run, force)
            }
            TickAction::Uninstall { dry_run } => service::tick_uninstall(dry_run),
            TickAction::Status => service::tick_status(),
        },
    }
}

/// Absolute path to the currently-running `ctx` binary, for baking into a
/// service unit. Falls back to the bare name if it can't be resolved.
fn current_ctx_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "ctx".to_string())
}

// -- Project command implementation --

/// Find the git repository root starting from `start`, or None when not
/// inside a git repo.
fn find_git_root(start: &std::path::Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let path = raw.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Current git branch via `git symbolic-ref` — deliberately no detached-
/// HEAD fallback, so mirroring never manufactures per-commit branches.
fn read_git_branch(dir: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let branch = raw.trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

/// Sanitize a git branch name into an ASG branch name. Must stay in sync
/// with `ctxone_hub::project::sanitize_branch_name` (the Hub records the
/// raw name as metadata precisely because this mapping is lossy).
fn sanitize_branch_name(raw: &str) -> String {
    let stripped = raw.strip_prefix("refs/heads/").unwrap_or(raw);
    let replaced: String = stripped
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut out = String::with_capacity(replaced.len());
    let mut last_was_dash = false;
    for c in replaced.chars() {
        if c == '-' {
            if !last_was_dash {
                out.push(c);
            }
            last_was_dash = true;
        } else {
            out.push(c);
            last_was_dash = false;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "work".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Read `git remote get-url origin` for detection registration.
fn read_git_remote(dir: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let url = raw.trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// Write the `.ctxproject` marker (project id, one line) at `root`.
fn write_ctxproject(root: &std::path::Path, id: &str) -> std::io::Result<PathBuf> {
    let path = root.join(".ctxproject");
    std::fs::write(&path, format!("{}\n", id))?;
    Ok(path)
}

/// Dispatch for `ctx project <subcommand>`.
async fn handle_project(
    action: ProjectAction,
    server: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        ProjectAction::Add {
            id,
            display_name,
            path,
            no_marker,
        } => {
            let cwd = std::env::current_dir()?;
            let root = path
                .map(PathBuf::from)
                .or_else(|| find_git_root(&cwd))
                .unwrap_or(cwd);
            let mut body = serde_json::json!({
                "id": id,
                "local_path": root.to_string_lossy(),
            });
            if let Some(d) = display_name {
                body["display_name"] = serde_json::json!(d);
            }
            if let Some(remote) = read_git_remote(&root) {
                body["remote_url"] = serde_json::json!(remote);
            }
            let resp = match client
                .post(format!("{}/api/projects", server))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "project add failed").await;
            }
            let mut parsed: Value = resp.json().await?;
            if !no_marker {
                let marker = write_ctxproject(&root, &id)?;
                parsed["marker"] = serde_json::json!(marker.to_string_lossy());
            }
            emit(format, &parsed, |v| {
                println!(
                    "Registered project {} (namespace: {})",
                    v["id"].as_str().unwrap_or("?"),
                    v["namespace"].as_str().unwrap_or("?"),
                );
                println!("  path: {}", root.display());
                if let Some(m) = v.get("marker").and_then(|m| m.as_str()) {
                    println!("  marker: {} (commit this so agents auto-detect)", m);
                }
            });
        }
        ProjectAction::List => {
            let resp = match client.get(format!("{}/api/projects", server)).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "project list failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let items = v.as_array().cloned().unwrap_or_default();
                if items.is_empty() {
                    println!("No projects registered. Run `ctx project add <id>` in a repo.");
                    return;
                }
                for p in items {
                    println!(
                        "{:<24} ns:{:<24} {}",
                        p["id"].as_str().unwrap_or("?"),
                        p["namespace"].as_str().unwrap_or("?"),
                        p["remote_url"].as_str().unwrap_or("-"),
                    );
                }
            });
        }
        ProjectAction::Use { id, no_marker } => {
            // Verify the project exists before touching the filesystem.
            let resp = match client
                .get(format!("{}/api/projects/{}", server, id))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "project use failed").await;
            }
            let cwd = std::env::current_dir()?;
            let root = find_git_root(&cwd).unwrap_or(cwd);
            // Bind this checkout's path so path-based lookups work too.
            let _ = client
                .post(format!("{}/api/projects/{}/paths", server, id))
                .json(&serde_json::json!({ "local_path": root.to_string_lossy() }))
                .send()
                .await;
            let mut parsed: Value = resp.json().await?;
            if !no_marker {
                let marker = write_ctxproject(&root, &id)?;
                parsed["marker"] = serde_json::json!(marker.to_string_lossy());
            }
            emit(format, &parsed, |v| {
                println!(
                    "This checkout now uses project {} (namespace: {})",
                    v["id"].as_str().unwrap_or("?"),
                    v["namespace"].as_str().unwrap_or("?"),
                );
            });
        }
        ProjectAction::Detect => {
            let cwd = std::env::current_dir()?;
            let resp = match client
                .get(format!("{}/api/projects/detect", server))
                .query(&[("cwd", cwd.to_string_lossy().as_ref())])
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "project detect failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| match v["status"].as_str() {
                Some("found") => println!(
                    "Project {} (namespace: {}, via {})",
                    v["project_id"].as_str().unwrap_or("?"),
                    v["namespace"].as_str().unwrap_or("?"),
                    v["via"].as_str().unwrap_or("?"),
                ),
                _ => println!(
                    "No project here — operating in the 'default' namespace. \
                     Run `ctx project add <id>` to give this repo its own."
                ),
            });
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
    let resp = match client.get(&url).send().await {
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
async fn agents_remove(
    server: &str,
    branch: &str,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let paths_url = format!(
        "{}/api/state/{}/paths?prefix=/memory/pinned/{}",
        server, branch, AGENTS_SOURCE
    );
    let paths_resp = match client.get(&paths_url).send().await {
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

    let resp = match client
        .clone()
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

pub(crate) fn urlencoding(s: &str) -> String {
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

/// Which MCP transport `ctx init` writes into a tool's config.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum McpTransport {
    /// Connect to a shared daemon's `/mcp` URL (Streamable HTTP). Default.
    Http,
    /// Spawn a per-tool `ctxone-hub` child over stdio (owns the db). Escape hatch.
    Stdio,
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
    client: reqwest::Client,
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
        match client.get(&url).send().await {
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
    let client = reqwest::Client::new();
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
    let hub_reachable = client.get(format!("{}/api/health", cli.server)).send()
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
        client.get(format!("{}/api/stats/main", cli.server)).send()
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

    // -- db-safety checks (added by t-007 of the db-safety plan) --

    // Common dev locations to scan. We don't recurse — these are the
    // exact spots where ctxone.db has historically appeared.
    let mut candidate_paths: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidate_paths.push(cwd.join("ctxone.db"));
        candidate_paths.push(cwd.join("target").join("ctxone.db"));
    }
    if let Some(home) = dirs::home_dir() {
        candidate_paths.push(home.join(".ctxone").join("memory.db"));
    }
    candidate_paths.push(std::path::PathBuf::from(&db));

    // Dedupe (canonicalize where possible).
    let mut seen = std::collections::HashSet::new();
    candidate_paths.retain(|p| {
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        seen.insert(key)
    });

    // Check 6 (a): inode drift — for each candidate <db>.lock with a
    // live PID, the corresponding db file must exist. If the lock is
    // present and the PID is alive but the db is gone, the hub is
    // writing to an unlinked inode (the 2026-04-28 failure mode).
    let mut drift_problems: Vec<String> = Vec::new();
    for p in &candidate_paths {
        // PathBuf::with_extension replaces rather than appends, so build
        // the `<db>.lock` path manually to match server/src/lockfile.rs.
        let lock = std::path::PathBuf::from(format!("{}.lock", p.display()));
        if !lock.exists() {
            continue;
        }
        let body = std::fs::read_to_string(&lock).unwrap_or_default();
        let pid: Option<u32> = body
            .split("\"pid\":")
            .nth(1)
            .map(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .and_then(|s| s.parse().ok());
        let alive = pid
            .map(|p| {
                std::process::Command::new("kill")
                    .args(["-0", &p.to_string()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if alive && !p.exists() {
            drift_problems.push(format!(
                "{} (lock pid {} alive, db missing)",
                p.display(),
                pid.unwrap_or(0)
            ));
        }
    }
    let drift_ok = drift_problems.is_empty();
    checks.push((
        "db inode drift".to_string(),
        drift_ok,
        if drift_ok {
            "no live hubs with missing db files".to_string()
        } else {
            drift_problems.join("; ")
        },
    ));
    if !drift_ok {
        suggestions.push(
            "Restart the hub immediately and restore from <db>.bak.<utc> — \
             writes are hitting an orphaned inode and will be lost on next restart"
                .to_string(),
        );
    }

    // Check 7 (b): multiple ctxone.db files in dev locations. One is
    // the canonical home; more than one means somebody (often us) ran
    // the hub from the wrong cwd and birthed a stub.
    let stray_paths: Vec<std::path::PathBuf> = candidate_paths
        .iter()
        .filter(|p| p.exists())
        .cloned()
        .collect();
    let stray_ok = stray_paths.len() <= 1;
    checks.push((
        "stray db files".to_string(),
        stray_ok,
        if stray_ok {
            format!("{} db file present", stray_paths.len())
        } else {
            format!(
                "{} db files in dev locations: {}",
                stray_paths.len(),
                stray_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
    ));
    if !stray_ok {
        suggestions.push(
            "Multiple ctxone.db files exist — confirm which one the hub is using \
             (check the --path arg) and remove the stragglers"
                .to_string(),
        );
    }

    // Check 8 (c): at least one snapshot from the last 24h. Looks for
    // <db>.bak.* siblings of each existing candidate db.
    let now = std::time::SystemTime::now();
    let one_day = std::time::Duration::from_secs(86_400);
    let mut recent_count = 0usize;
    for p in &stray_paths {
        let parent = p.parent().unwrap_or_else(|| std::path::Path::new("."));
        let basename = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prefix = format!("{}.bak.", basename);
        if let Ok(entries) = std::fs::read_dir(parent) {
            for e in entries.flatten() {
                if !e.file_name().to_string_lossy().starts_with(&prefix) {
                    continue;
                }
                if let Ok(meta) = e.metadata()
                    && let Ok(mtime) = meta.modified()
                    && now
                        .duration_since(mtime)
                        .map(|d| d < one_day)
                        .unwrap_or(false)
                {
                    recent_count += 1;
                }
            }
        }
    }
    let backups_ok = !stray_paths.is_empty() && recent_count > 0;
    checks.push((
        "recent backups".to_string(),
        backups_ok || stray_paths.is_empty(),
        if stray_paths.is_empty() {
            "no db files to back up".to_string()
        } else if backups_ok {
            format!("{} snapshot(s) within last 24h", recent_count)
        } else {
            "no .bak.* siblings within last 24h".to_string()
        },
    ));
    if !stray_paths.is_empty() && !backups_ok {
        suggestions.push(
            "Take a snapshot now: ctx db backup  (or start the hub — startup snapshots are automatic)"
                .to_string(),
        );
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

// ── ingest-session / capture-turn ────────────────────────────────────────────

/// Derive a session id from a JSONL transcript path. Claude Code names its
/// session files `<uuid>.jsonl`, so the file stem makes a stable, unique
/// session id that lets each ingested file land as its own row on the
/// Sessions page (with its own token totals and tagged memories).
///
/// Falls back to "default" if the path has no usable stem (shouldn't happen
/// in practice — Claude Code always names files).
fn session_id_for_file(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string())
}

#[allow(clippy::too_many_arguments)]
async fn run_ingest_session(
    server: &str,
    branch: &str,
    session: Option<&str>,
    file: Option<String>,
    all: bool,
    since: Option<String>,
    last: Option<usize>,
    tokens_only: bool,
    dry_run: bool,
    mut full_turn: bool,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    if api_key.is_empty() && !tokens_only {
        eprintln!(
            "warn: ANTHROPIC_API_KEY not set — memory extraction disabled (use --tokens-only to suppress)"
        );
    }

    // `--all` forces full-turn + token capture so a whole-machine sync always
    // rebuilds the Sessions view's turn/token data even with no API key.
    if all {
        full_turn = true;
    }

    // Build the (project-label, files) groups to ingest. A single explicit
    // --file or the cwd project are one-group cases; --all fans out across
    // every project under ~/.claude/projects so per-project counts print.
    let groups: Vec<(String, Vec<std::path::PathBuf>)> = if let Some(f) = file {
        vec![(String::new(), vec![std::path::PathBuf::from(f)])]
    } else if all {
        crate::ingest::find_all_session_files()
    } else {
        let cwd = std::env::current_dir()?;
        let label = cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        vec![(label, crate::ingest::find_session_files(&cwd))]
    };

    if groups.iter().all(|(_, f)| f.is_empty()) {
        if all {
            println!("No Claude Code session files found in ~/.claude/projects/");
            // Still emit the machine-readable summary so the hub's session-sync
            // parser always finds a final JSON line (zeros = clean no-op).
            println!("{}", serde_json::json!({ "sessions": 0, "turns": 0, "tokens": 0 }));
        } else {
            println!("No session files found for this project.");
            println!("Pass --file <path> to specify a .jsonl file directly.");
        }
        return Ok(());
    }

    // Parse since date filter.
    let since_ts = since.as_deref().unwrap_or("");

    let mut total_sessions = 0usize;
    let mut total_turns_seen = 0usize;
    let mut total_memories = 0usize;
    let mut total_full_turns = 0usize;
    let mut total_tokens = crate::ingest::TurnTokens::default();

    for (label, files) in &groups {
        if files.is_empty() {
            continue;
        }
        if all && !label.is_empty() {
            println!("\n=== project: {} ({} files) ===", label, files.len());
        }
        let mut proj_sessions = 0usize;
        let mut proj_turns = 0usize;
        let mut proj_tokens = crate::ingest::TurnTokens::default();

        for path in files {
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            // If the caller didn't pin an explicit --session, give each file
            // its own session id derived from the filename so the Sessions
            // page shows one row per ingested transcript instead of
            // collapsing everything into "default".
            let derived_sid;
            let effective_session: Option<&str> = match session {
                Some(s) => Some(s),
                None => {
                    derived_sid = session_id_for_file(path);
                    Some(derived_sid.as_str())
                }
            };
            println!(
                "→ {}  (session: {})",
                fname,
                effective_session.unwrap_or("default")
            );

            let mut turns = crate::ingest::parse_turns(path);
            if !turns.is_empty() {
                proj_sessions += 1;
            }

            // Session title (t-016): derive from the FULL session (before the
            // --since/--last filters below) so it reflects where the session
            // started, then persist a title node at /sessions/{id}/title.
            // Fallback: "<project-label> · <date>".
            let title = crate::ingest::derive_session_title(&turns).unwrap_or_else(|| {
                let proj = if !label.is_empty() {
                    label.clone()
                } else {
                    std::env::current_dir()
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                        .unwrap_or_else(|| "session".to_string())
                };
                let date = turns
                    .first()
                    .map(|t| t.timestamp.clone())
                    .filter(|ts| ts.len() >= 10)
                    .map(|ts| ts[..10].to_string())
                    .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
                crate::ingest::truncate_title(&format!("{} · {}", proj, date))
            });
            if dry_run {
                println!(
                    "  [dry] title: /sessions/{}/title = {:?}",
                    effective_session.unwrap_or("default"),
                    title
                );
            } else {
                crate::ingest::store_session_title(
                    &title,
                    server,
                    branch,
                    effective_session,
                    &client,
                )
                .await;
            }

            // Session meta (t-021): source + first/last turn timestamps, so the
            // Lens can filter by agent type and sort by date. `ctx
            // ingest-session` only parses Claude Code transcripts today, so the
            // source is "Claude Code"; Cursor/Copilot ingesters would set their
            // own. Timestamps come from the full (pre-filter) turn list.
            let started_at = turns.first().map(|t| t.timestamp.clone()).unwrap_or_default();
            let updated_at = turns.last().map(|t| t.timestamp.clone()).unwrap_or_default();
            // Distinct real models across all turns, first-seen order — so a
            // session that switched models mid-way stays findable by any.
            // Skip synthetic/placeholder markers (e.g. "<synthetic>" on
            // system/tool-result turns) and empties.
            let mut models_used: Vec<String> = Vec::new();
            for t in &turns {
                let m = t.model.trim();
                let real = !m.is_empty() && !m.starts_with('<');
                if real && !models_used.iter().any(|x| x == m) {
                    models_used.push(m.to_string());
                }
            }
            if dry_run {
                println!(
                    "  [dry] meta: /sessions/{}/meta = {{source: \"Claude Code\", started_at: {:?}, updated_at: {:?}, models_used: {:?}}}",
                    effective_session.unwrap_or("default"),
                    started_at,
                    updated_at,
                    models_used
                );
            } else {
                crate::ingest::store_session_meta(
                    "Claude Code",
                    &started_at,
                    &updated_at,
                    &models_used,
                    server,
                    branch,
                    effective_session,
                    &client,
                )
                .await;
            }

            // Apply --since filter on timestamp.
            if !since_ts.is_empty() {
                turns.retain(|t| t.timestamp.as_str() >= since_ts);
            }

            // Apply --last filter.
            if let Some(n) = last {
                let skip = turns.len().saturating_sub(n);
                turns = turns.into_iter().skip(skip).collect();
            }

            let source_file = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            for (idx, turn) in turns.iter().enumerate() {
                proj_turns += 1;
                proj_tokens.add(&turn.tokens);

                if !turn.tokens.is_empty() {
                    if dry_run {
                        println!(
                            "  [dry] token record: in={} out={} cache_read={} cache_create={} model={}",
                            turn.tokens.input,
                            turn.tokens.output,
                            turn.tokens.cache_read,
                            turn.tokens.cache_creation,
                            turn.model,
                        );
                    } else {
                        crate::ingest::record_turn_tokens(
                            &turn.tokens,
                            &turn.model,
                            server,
                            effective_session,
                            &client,
                        )
                        .await;
                    }
                }

                if full_turn {
                    if dry_run {
                        println!(
                            "  [dry] full turn: /sessions/{}/turns/{:04} ({} bytes assistant, {} tools)",
                            effective_session.unwrap_or("default"),
                            idx,
                            turn.assistant_text.len(),
                            turn.tool_calls_raw.len(),
                        );
                    } else {
                        crate::ingest::store_full_turn(
                            turn,
                            idx,
                            &source_file,
                            server,
                            branch,
                            effective_session,
                            &client,
                        )
                        .await;
                        total_full_turns += 1;
                    }
                }

                if tokens_only || api_key.is_empty() || !turn.is_substantial() {
                    continue;
                }

                let memories = crate::ingest::extract_memories(turn, &api_key, &client).await;
                for mem in &memories {
                    if dry_run {
                        println!(
                            "  [dry] memory: {} ({}) — {}",
                            mem.path, mem.importance, mem.title
                        );
                    } else {
                        crate::ingest::store_memory(mem, server, branch, effective_session, &client)
                            .await;
                        print!(".");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                }
                if !memories.is_empty() && !dry_run {
                    println!(" {} memories", memories.len());
                }
                total_memories += memories.len();
            }
        }

        if all && !label.is_empty() {
            println!(
                "  project {}: {} sessions, {} turns, {} tokens",
                label,
                proj_sessions,
                proj_turns,
                proj_tokens.input
                    + proj_tokens.output
                    + proj_tokens.cache_read
                    + proj_tokens.cache_creation,
            );
        }
        total_sessions += proj_sessions;
        total_turns_seen += proj_turns;
        total_tokens.add(&proj_tokens);
    }

    let grand_tokens =
        total_tokens.input + total_tokens.output + total_tokens.cache_read + total_tokens.cache_creation;

    println!();
    println!(
        "Done. {} sessions, {} turns processed, {} memories stored, {} full turns persisted.",
        total_sessions, total_turns_seen, total_memories, total_full_turns
    );
    println!(
        "Tokens — input: {}  output: {}  cache_read: {}  cache_create: {}",
        total_tokens.input,
        total_tokens.output,
        total_tokens.cache_read,
        total_tokens.cache_creation,
    );

    // Machine-readable final line so a caller (the hub's session-sync
    // endpoint) can parse the outcome without scraping prose. Always the LAST
    // stdout line under --all.
    if all {
        println!(
            "{}",
            serde_json::json!({
                "sessions": total_sessions,
                "turns": total_turns_seen,
                "tokens": grand_tokens,
            })
        );
    }
    Ok(())
}

// ── ctx session metrics ───────────────────────────────────────────────────────

async fn run_session_action(action: SessionAction) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        SessionAction::Metrics {
            project,
            session,
            list,
            all,
            json,
            gap,
            verbose,
        } => {
            run_session_metrics(project, session, list, all, json, gap, verbose).await?;
        }
    }
    Ok(())
}

async fn run_session_metrics(
    project: Option<String>,
    session_filter: Option<String>,
    list: bool,
    all: bool,
    json_out: bool,
    gap: f64,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::metrics::{
        SessionMetrics, all_project_sessions, find_session_files, fmt_tokens,
        parse_session_metrics, render_list_row, render_metrics,
    };

    if all {
        let projects = all_project_sessions();
        if projects.is_empty() {
            println!("No Claude Code session files found in ~/.claude/projects/");
            return Ok(());
        }

        let mut all_sessions: Vec<(String, Vec<SessionMetrics>)> = vec![];
        for (label, files) in &projects {
            let sessions: Vec<SessionMetrics> = files
                .iter()
                .map(|f| parse_session_metrics(f, gap))
                .filter(|s| s.turns > 0)
                .collect();
            if !sessions.is_empty() {
                all_sessions.push((label.clone(), sessions));
            }
        }

        if json_out {
            println!("{}", serde_json::to_string_pretty(&all_sessions)?);
            return Ok(());
        }

        let mut grand_total = SessionMetrics::default();
        for (label, sessions) in &all_sessions {
            let bar = "─".repeat(62);
            println!("\n{}", bar);
            println!("  Project: {}", label);
            println!("{}", bar);
            if list {
                for sm in sessions {
                    render_list_row(sm);
                }
            }
            let mut project_total = SessionMetrics::default();
            for sm in sessions {
                project_total.add(sm);
            }
            let savings_pct = if project_total.cost_no_cache_usd > 0.0 {
                project_total.cache_savings_usd / project_total.cost_no_cache_usd * 100.0
            } else {
                0.0
            };
            println!(
                "  {} sessions  {} turns  {:.1}% cache  ${:.2} actual  (saved {:.0}% vs no-cache ${:.2})",
                sessions.len(),
                project_total.turns,
                project_total.cache_hit_rate * 100.0,
                project_total.cost_usd,
                savings_pct,
                project_total.cost_no_cache_usd,
            );
            grand_total.add(&project_total);
        }

        let grand_savings_pct = if grand_total.cost_no_cache_usd > 0.0 {
            grand_total.cache_savings_usd / grand_total.cost_no_cache_usd * 100.0
        } else {
            0.0
        };
        println!("\n{}", "═".repeat(70));
        println!(
            "  TOTAL  {} projects  {} turns  {:.1}% cache",
            all_sessions.len(),
            grand_total.turns,
            grand_total.cache_hit_rate * 100.0,
        );
        println!(
            "         Actual cost:   ${:.2}  (per-turn pricing with cache discount applied)",
            grand_total.cost_usd,
        );
        println!(
            "         Without cache: ${:.2}  (saved ${:.2}  = {:.1}% reduction)",
            grand_total.cost_no_cache_usd, grand_total.cache_savings_usd, grand_savings_pct,
        );
        println!("{}", "═".repeat(70));
        return Ok(());
    }

    // Single project
    let project_dir = project.map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let files = find_session_files(&project_dir);
    if files.is_empty() {
        println!("No session files found for: {}", project_dir.display());
        println!("  (expected ~/.claude/projects/<hash>/*.jsonl)");
        return Ok(());
    }

    // Apply session filter
    let files: Vec<_> = if let Some(ref sid) = session_filter {
        files
            .into_iter()
            .filter(|f| {
                f.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.starts_with(sid.as_str()))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        files
    };

    if files.is_empty() {
        println!("No sessions match that filter.");
        return Ok(());
    }

    let sessions: Vec<SessionMetrics> = files
        .iter()
        .map(|f| parse_session_metrics(f, gap))
        .filter(|s| s.turns > 0)
        .collect();

    let label = project_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();

    if list {
        let bar = "─".repeat(62);
        println!("\n{}", bar);
        println!("  Sessions — {}", label);
        println!("{}", bar);
        println!(
            "  {:8}  {:16}  {:9}  {:8}  {:7}  {:11}  {}",
            "UUID", "Started", "Turns", "Input", "Output", "Cache hit%", "Cost"
        );
        for sm in &sessions {
            render_list_row(sm);
        }
        let mut total = SessionMetrics::default();
        for sm in &sessions {
            total.add(sm);
        }
        println!("{}", bar);
        println!(
            "  Total: {} sessions, {} turns, ${:.4}",
            sessions.len(),
            total.turns,
            total.cost_usd
        );
        return Ok(());
    }

    if json_out {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }

    // Show per-session detail only when a filter is active; otherwise show aggregate.
    if session_filter.is_some() {
        for sm in &sessions {
            render_metrics(sm, &sm.session_id, gap, verbose);
        }
    } else {
        let mut total = SessionMetrics::default();
        for sm in &sessions {
            total.add(sm);
        }
        total.session_id = format!("{} sessions", sessions.len());
        render_metrics(&total, &label, gap, verbose);
    }

    Ok(())
}

async fn run_capture_turn(
    server: &str,
    branch: &str,
    session: Option<&str>,
    transcript: Option<String>,
    turns: usize,
    tokens_only: bool,
    full_turn: bool,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    // Resolve transcript path: explicit flag > stdin hook payload > latest session file.
    let transcript_path: std::path::PathBuf = if let Some(t) = transcript {
        std::path::PathBuf::from(t)
    } else {
        // Try reading hook payload from stdin (non-blocking check).
        let stdin_payload = read_stdin_nonblocking();
        if let Some(path) = stdin_payload
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| {
                v.get("transcript_path")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string())
            })
        {
            std::path::PathBuf::from(path)
        } else {
            // Fall back to the most recent session file for the cwd.
            let cwd = std::env::current_dir()?;
            match crate::ingest::latest_session_file(&cwd) {
                Some(p) => p,
                None => {
                    // Silent exit — hook context, not interactive.
                    return Ok(());
                }
            }
        }
    };

    if !transcript_path.exists() {
        return Ok(());
    }

    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    // last_turns gives us tail items; for the full-turn path we need their
    // absolute index in the session so paths stay stable across captures.
    let all_count = crate::ingest::parse_turns(&transcript_path).len();
    let recent = crate::ingest::last_turns(&transcript_path, turns);
    let base_idx = all_count.saturating_sub(recent.len());
    let source_file = transcript_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    // If no explicit --session was passed, derive one from the transcript
    // filename (Claude Code names files <uuid>.jsonl) so each captured
    // session lands as its own row on the Sessions page.
    let derived_sid = session_id_for_file(&transcript_path);
    let effective_session: Option<&str> = match session {
        Some(s) => Some(s),
        None => Some(derived_sid.as_str()),
    };

    for (offset, turn) in recent.iter().enumerate() {
        let idx = base_idx + offset;
        if !turn.tokens.is_empty() {
            crate::ingest::record_turn_tokens(
                &turn.tokens,
                &turn.model,
                server,
                effective_session,
                &client,
            )
            .await;
        }
        if full_turn {
            crate::ingest::store_full_turn(
                turn,
                idx,
                &source_file,
                server,
                branch,
                effective_session,
                &client,
            )
            .await;
        }
        if tokens_only || api_key.is_empty() || !turn.is_substantial() {
            continue;
        }
        let memories = crate::ingest::extract_memories(turn, &api_key, &client).await;
        for mem in &memories {
            crate::ingest::store_memory(mem, server, branch, effective_session, &client).await;
        }
    }

    Ok(())
}

/// Read up to 4KB from stdin with a short timeout (for hook payloads).
/// Returns None immediately if stdin has no data (interactive mode).
fn read_stdin_nonblocking() -> Option<String> {
    use std::io::Read;
    // Only attempt to read stdin if it's not a TTY (i.e., hook piped data).
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return None;
    }
    let mut buf = String::new();
    let _ = std::io::stdin().take(4096).read_to_string(&mut buf);
    if buf.trim().is_empty() {
        None
    } else {
        Some(buf)
    }
}

async fn run_demo(server: &str, client: reqwest::Client) -> Result<(), Box<dyn std::error::Error>> {
    // Verify Hub is reachable first
    match client.get(format!("{}/api/health", server)).send().await {
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
        let resp = client.get(format!(
            "{}/api/memory/recall?topic={}&budget={}",
            server,
            urlencoding(topic),
            budget
        )).send()
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
    if let Ok(resp) = client.get(format!("{}/api/stats/tokens", server)).send().await
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

/// Resolve the stable `CTX_SESSION` id baked into the stdio MCP server's env
/// (t-015). This unifies a project's MCP-side memory savings into ONE session
/// row in the Hub's `ctxone_sessions` table, and is stable across re-inits.
///
/// Preference order:
///   1. the registered project namespace (when `detect_project` matched) — so
///      every tool configured for this repo converges on the same id, and it
///      lines up with where the stdio server scopes its writes; else
///   2. a deterministic hash of the canonical project directory. `DefaultHasher`
///      uses fixed keys, so the same path always yields the same id across
///      processes and re-inits, with no file to persist and no divergence
///      between per-tool config files.
///
/// LIMITATION (documented on purpose): this keys savings per PROJECT, not per
/// Claude Code CONVERSATION. Claude Code's own session ids are per-conversation
/// UUIDs (the `<uuid>.jsonl` stem that `ctx ingest-session` uses); aligning MCP
/// savings with those would require a per-turn hook injecting the live
/// conversation id into CTX_SESSION, which is out of scope here. So `ctx
/// ingest-session` rows (per conversation) and live MCP rows (per project) live
/// side by side rather than merging.
fn init_session_id(namespace: Option<&str>) -> String {
    if let Some(ns) = namespace {
        let ns = ns.trim();
        if !ns.is_empty() && ns != "default" {
            return ns.to_string();
        }
    }
    use std::hash::{Hash, Hasher};
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cwd.hash(&mut h);
    format!("proj-{:016x}", h.finish())
}

fn mcp_server_entry(
    agent_id: &str,
    client_name: &str,
    transport: McpTransport,
    mcp_url: &str,
    namespace: Option<&str>,
    auth_token: Option<&str>,
    auth_token_env: Option<&str>,
) -> Value {
    match transport {
        McpTransport::Http => {
            // Point the tool at a shared daemon's Streamable-HTTP endpoint.
            // The daemon scopes writes by the `?namespace=` query, so bake in
            // the detected namespace.
            let url = mcp_http_url(mcp_url, namespace);
            if http_client_needs_bridge(client_name) {
                // Stdio-only JSON clients (Claude Desktop) can't read
                // `{"type":"http","url":…}`. Bridge to the HTTP hub with
                // `mcp-remote`, a stdio proxy launched via npx. `-y` skips the
                // install prompt; `--transport http-only` forces Streamable
                // HTTP (no SSE fallback probe).
                let mut args = vec![
                    "-y".to_string(),
                    "mcp-remote".to_string(),
                    url,
                    "--transport".to_string(),
                    "http-only".to_string(),
                ];
                // mcp-remote forwards a literal --header; it does not expand
                // env vars, so only a literal token works for the bridge.
                if let Some(tok) = auth_token {
                    args.push("--header".to_string());
                    args.push(format!("Authorization: Bearer {tok}"));
                }
                serde_json::json!({ "command": "npx", "args": args })
            } else {
                // Native URL transport (Claude Code, Cursor, VS Code).
                let mut entry = serde_json::json!({ "type": "http", "url": url });
                // Prefer a literal header; else reference an env var (clients
                // that support `${VAR}` expansion in config values resolve it).
                let auth_value = auth_token
                    .map(|t| format!("Bearer {t}"))
                    .or_else(|| auth_token_env.map(|v| format!("Bearer ${{{v}}}")));
                if let Some(v) = auth_value {
                    entry["headers"] = serde_json::json!({ "Authorization": v });
                }
                entry
            }
        }
        McpTransport::Stdio => {
            let hub_bin = find_hub_binary();
            let db_path = canonical_db_path();

            // Ensure the parent directory exists so the Hub can create the db
            // on first run.
            if let Some(parent) = std::path::Path::new(&db_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            serde_json::json!({
                "command": hub_bin,
                "args": ["--path", db_path, "--agent-id", agent_id]
            })
        }
    }
}

/// JSON MCP clients that only speak stdio and therefore need the `mcp-remote`
/// bridge to reach an HTTP hub. Claude Desktop has no native `{type:http,url}`
/// support (verified 2026-07, app v1.2x) — it ignores such entries — so under
/// `--transport http` we write an `mcp-remote` stdio proxy for it instead.
fn http_client_needs_bridge(client_name: &str) -> bool {
    client_name == "Claude Desktop"
}

/// Derive the hub's health URL (`<scheme>://<authority>/api/health`) from an
/// `/mcp` endpoint URL, so `ctx init --transport http` can preflight the daemon.
/// Returns `None` if `base` has no scheme+authority.
fn hub_health_url(base: &str) -> Option<String> {
    let (scheme, rest) = base.split_once("://")?;
    let authority = rest.split(['/', '?']).next().filter(|a| !a.is_empty())?;
    Some(format!("{scheme}://{authority}/api/health"))
}

/// Compose the `/mcp` URL, appending `namespace=<ns>` to whatever query the
/// base URL already carries (so an explicit `--mcp-url …?foo=bar` is preserved).
fn mcp_http_url(base: &str, namespace: Option<&str>) -> String {
    match namespace {
        Some(ns) if !ns.is_empty() => {
            let sep = if base.contains('?') { '&' } else { '?' };
            format!("{base}{sep}namespace={ns}")
        }
        _ => base.to_string(),
    }
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
    session_id: &str,
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
    // CTX_SESSION env (t-015): stable per-project id so the stdio hub persists
    // its recall/remember savings under one session row.
    let mut env = toml::map::Map::new();
    env.insert(
        "CTX_SESSION".to_string(),
        Value::String(session_id.to_string()),
    );
    ctxone.insert("env".to_string(), Value::Table(env));

    servers.insert("ctxone".to_string(), Value::Table(ctxone));

    toml::to_string_pretty(&doc).map_err(|e| format!("serialize failed: {}", e))
}

/// Merge a Streamable-HTTP `[mcp_servers.ctxone]` entry into a Codex TOML
/// config. Codex supports HTTP MCP natively via a `url` key (verified against
/// the official docs; no `experimental_use_rmcp_client` needed on current
/// versions). Any stale stdio keys from a prior stdio install are removed so
/// the entry doesn't carry both `command` and `url`.
fn merge_codex_ctxone_toml_http(
    existing: &str,
    url: &str,
    auth_token_env: Option<&str>,
) -> Result<String, String> {
    use toml::Value;

    let mut doc: Value = if existing.trim().is_empty() {
        Value::Table(toml::map::Map::new())
    } else {
        toml::from_str(existing).map_err(|e| format!("invalid existing TOML: {}", e))?
    };

    let root = doc
        .as_table_mut()
        .ok_or_else(|| "config root is not a table".to_string())?;

    let servers = root
        .entry("mcp_servers".to_string())
        .or_insert_with(|| Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or_else(|| "mcp_servers is not a table".to_string())?;

    let mut ctxone = toml::map::Map::new();
    ctxone.insert("url".to_string(), Value::String(url.to_string()));
    // Codex sources the bearer from an env var at runtime (never a literal).
    if let Some(var) = auth_token_env {
        ctxone.insert(
            "bearer_token_env_var".to_string(),
            Value::String(var.to_string()),
        );
    }

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

#[allow(clippy::too_many_arguments)]
fn init_mcp(
    global: bool,
    tool_filter: Option<String>,
    generic_config_path: Option<String>,
    dry_run: bool,
    transport: McpTransport,
    mcp_url: &str,
    namespace: Option<String>,
    auth_token: Option<&str>,
    auth_token_env: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tools = detect_tools(global);

    // Stable per-project CTX_SESSION id for the stdio MCP server (t-015).
    // Injected as `env.CTX_SESSION` below so the spawned hub attributes and
    // PERSISTS its recall/remember savings under one id instead of losing them
    // to a never-flushed default session. See `init_session_id` for the
    // per-project (not per-conversation) limitation.
    let session_id = init_session_id(namespace.as_deref());

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
        let mut entry = mcp_server_entry(
            &agent_id,
            t.name,
            transport,
            mcp_url,
            namespace.as_deref(),
            auth_token,
            auth_token_env,
        );

        // Inject CTX_SESSION into the stdio server's env (t-015) so its
        // recall/remember savings persist under a stable id. Only the stdio
        // transport spawns a per-tool hub process; the HTTP transport scopes
        // by the `?namespace=` URL and the X-CTXone-Session header instead.
        if transport == McpTransport::Stdio
            && let Some(obj) = entry.as_object_mut()
        {
            obj.insert(
                "env".to_string(),
                serde_json::json!({ "CTX_SESSION": session_id }),
            );
        }

        // Tell the user when http mode falls back to the mcp-remote bridge, so
        // they know a Node/npx runtime is now a prerequisite for that client.
        if transport == McpTransport::Http && http_client_needs_bridge(t.name) {
            eprintln!(
                "  \u{2139} {}: no native HTTP MCP support; using the `mcp-remote` \
                 stdio bridge (requires Node/npx).",
                t.name
            );
        }

        // Per-client auth caveats under http. Codex needs an env-var name; the
        // mcp-remote bridge needs a literal token (no env expansion); and a
        // literal token is written into the config file in plaintext.
        if transport == McpTransport::Http {
            let is_codex = matches!(t.config_type, ConfigType::Toml);
            if is_codex && auth_token_env.is_none() && auth_token.is_some() {
                eprintln!(
                    "  \u{26A0} {}: Codex reads the token from an env var — pass \
                     --auth-token-env <VAR> (and export it); a literal --auth-token \
                     can't be embedded, so no token was written.",
                    t.name
                );
            } else if http_client_needs_bridge(t.name)
                && auth_token.is_none()
                && auth_token_env.is_some()
            {
                eprintln!(
                    "  \u{26A0} {}: the mcp-remote bridge can't expand an env var — \
                     pass a literal --auth-token <TOK> to authenticate; none was written.",
                    t.name
                );
            } else if auth_token.is_some() && !is_codex {
                eprintln!(
                    "  \u{26A0} {}: bearer token written in plaintext into {}.",
                    t.name,
                    t.config_path.display()
                );
            }
        }

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
                let db_path = canonical_db_path();
                if let Some(parent) = std::path::Path::new(&db_path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                let existing = if t.config_path.exists() {
                    std::fs::read_to_string(&t.config_path).unwrap_or_default()
                } else {
                    String::new()
                };

                // Codex supports both transports natively: stdio (command/args)
                // and Streamable HTTP (a `url` key). Pick per --transport.
                let merged = match transport {
                    McpTransport::Http => {
                        let url = mcp_http_url(mcp_url, namespace.as_deref());
                        merge_codex_ctxone_toml_http(&existing, &url, auth_token_env)
                    }
                    McpTransport::Stdio => merge_codex_ctxone_toml(
                        &existing,
                        &find_hub_binary(),
                        &db_path,
                        &agent_id,
                        &session_id,
                    ),
                };
                let new_content = match merged {
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
            force,
        } => {
            let mut body = serde_json::json!({
                "title": title,
                "priority": priority,
                "ref": branch,
            });
            if force {
                body["force"] = serde_json::json!(true);
            }
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
            // Non-blocking warning when other tasks in the plan are already
            // in progress (the server attaches it as a `warning` field).
            if let Some(w) = parsed["warning"].as_str() {
                eprintln!("  \u{26A0} {}", w);
            }
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
                // Nudge: this task satisfies task(s) in other plans.
                if let Some(sat) = v["satisfies"].as_array()
                    && !sat.is_empty()
                {
                    let targets: Vec<&str> = sat.iter().filter_map(|x| x.as_str()).collect();
                    println!(
                        "  \u{21B3} satisfies {} — mark it done too if complete",
                        targets.join(", ")
                    );
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
            in_order,
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
            if in_order {
                parts.push("mode=order".to_string());
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
            emit(format, &parsed, |v| {
                match v.get("task") {
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
                }
                // Show active work separately from the next unstarted task.
                if let Some(active) = v["in_progress"].as_array()
                    && !active.is_empty()
                {
                    println!("In progress:");
                    for t in active {
                        println!(
                            "  {} {}",
                            t["id"].as_str().unwrap_or(""),
                            t["title"].as_str().unwrap_or("")
                        );
                    }
                }
            });
        }
        PlanAction::List {
            status,
            all_namespaces,
        } => {
            let mut url = format!("{}/api/plans?ref={}", server, urlencoding(branch));
            if let Some(s) = status {
                url.push_str(&format!("&status={}", urlencoding(&s)));
            }
            if all_namespaces {
                url.push_str("&all_namespaces=true");
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
                    // In --all-namespaces mode the server tags each plan with its
                    // namespace; prefix it so the inventory is unambiguous.
                    let ns_prefix = plan["namespace"]
                        .as_str()
                        .map(|n| format!("[{n}] "))
                        .unwrap_or_default();
                    println!(
                        "{}{:<24} {:<10} {} tasks [{}✓ {}→ {} ]",
                        ns_prefix, name, status, total, done, in_progress, pending
                    );
                }
            });
        }
        PlanAction::Link {
            plan_id,
            task_id,
            target,
        } => {
            let url = format!(
                "{}/api/plans/{}/tasks/{}/link",
                server,
                urlencoding(&plan_id),
                urlencoding(&task_id),
            );
            let body = serde_json::json!({ "ref": branch, "target": target });
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
                http_error_exit(resp, "plan link failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let sat: Vec<&str> = v["satisfies"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                    .unwrap_or_default();
                println!(
                    "{} {} now satisfies: {}",
                    plan_id,
                    task_id,
                    sat.join(", ")
                );
            });
        }
        PlanAction::Stale {
            days,
            all_namespaces,
        } => {
            let mut url = format!(
                "{}/api/plans/stale?ref={}&days={}",
                server,
                urlencoding(branch),
                days
            );
            if all_namespaces {
                url.push_str("&all_namespaces=true");
            }
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "plan stale failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let empty = vec![];
                let arr = v.as_array().unwrap_or(&empty);
                if arr.is_empty() {
                    println!("No stale in-progress tasks (older than {days}d).");
                    return;
                }
                println!("Stale in-progress tasks (>{days}d):");
                for t in arr {
                    let ns_prefix = t["namespace"]
                        .as_str()
                        .map(|n| format!("[{n}] "))
                        .unwrap_or_default();
                    println!(
                        "  {}{} {} ({}d) — {}",
                        ns_prefix,
                        t["plan"].as_str().unwrap_or(""),
                        t["id"].as_str().unwrap_or(""),
                        t["age_days"].as_i64().unwrap_or(0),
                        t["title"].as_str().unwrap_or("")
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
        PlanAction::Tasks { plan_id } => {
            // Mirrors the `plan_tasks` MCP tool: returns just the flat
            // task array, no plan envelope. Useful in scripts that don't
            // care about plan metadata.
            let url = format!(
                "{}/api/plans/{}/tasks?ref={}",
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
                http_error_exit(resp, "plan tasks failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let empty = vec![];
                let tasks = v.as_array().unwrap_or(&empty);
                if tasks.is_empty() {
                    println!("(no tasks)");
                    return;
                }
                for task in tasks {
                    let id = task["id"].as_str().unwrap_or("");
                    let title = task["title"].as_str().unwrap_or("");
                    let status = task["status"].as_str().unwrap_or("");
                    let pri = task["priority"].as_str().unwrap_or("");
                    let assigned = task["assigned_to"].as_str();
                    let mut line = format!(
                        "{} {} {} {}",
                        status_glyph(status),
                        id,
                        priority_tag(pri),
                        title
                    );
                    if let Some(a) = assigned {
                        line.push_str(&format!(" @{}", a));
                    }
                    println!("{}", line);
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
        PlanAction::Complete { plan_id, reason } => {
            let url = format!(
                "{}/api/plans/{}/force_complete?ref={}",
                server,
                urlencoding(&plan_id),
                urlencoding(&branch)
            );
            let body = serde_json::json!({ "reason": reason });
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
                http_error_exit(resp, "plan complete failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let plan_name = v["plan"]["name"].as_str().unwrap_or("");
                let abandoned = v["abandoned_task_ids"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0);
                println!(
                    "Force-completed plan {} ({} task{} abandoned)",
                    plan_name,
                    abandoned,
                    if abandoned == 1 { "" } else { "s" }
                );
                if let Some(arr) = v["abandoned_task_ids"].as_array() {
                    for id in arr {
                        if let Some(s) = id.as_str() {
                            println!("  - {}", s);
                        }
                    }
                }
            });
        }
        PlanAction::Move { plan_id, target } => {
            let url = format!(
                "{}/api/plans/{}/move?ref={}",
                server,
                urlencoding(&plan_id),
                urlencoding(&branch)
            );
            let body = serde_json::json!({ "target_ref": target });
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
                http_error_exit(resp, "plan move failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let plan_name = v["plan"]["name"].as_str().unwrap_or("");
                let count = v["task_count"].as_u64().unwrap_or(0);
                println!(
                    "Moved plan {} from {} to {} ({} task{})",
                    plan_name,
                    v["source_ref"].as_str().unwrap_or(""),
                    v["target_ref"].as_str().unwrap_or(""),
                    count,
                    if count == 1 { "" } else { "s" }
                );
            });
        }
    }
    Ok(())
}

async fn handle_taint(
    action: TaintAction,
    server: &str,
    branch: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let agent_id = std::env::var("CTX_AGENT_ID").unwrap_or_else(|_| "ctx-cli".to_string());
    let agent_id = agent_id.as_str();
    match action {
        TaintAction::List {
            path_prefix,
            kind,
            include_resolved,
        } => {
            let mut url = format!("{}/api/taint?include_resolved={}", server, include_resolved);
            if let Some(p) = &path_prefix {
                url.push_str(&format!("&path_prefix={}", urlencoding(p)));
            }
            if let Some(k) = &kind {
                url.push_str(&format!("&kind={}", urlencoding(k)));
            }
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "taint list failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let empty = vec![];
                let taints = v["taints"].as_array().unwrap_or(&empty);
                if taints.is_empty() {
                    println!("(no taints)");
                    return;
                }
                for t in taints {
                    let id = t["id"].as_str().unwrap_or("");
                    let kind = t["kind"].as_str().unwrap_or("?");
                    let name = t["name"].as_str().unwrap_or("");
                    let path = t["path"].as_str().unwrap_or("");
                    let sev = t["severity"].as_str().unwrap_or("");
                    let resolved = t["resolved_at"].as_str().is_some();
                    let mark = if resolved { "[resolved] " } else { "" };
                    println!("{}{} {} {} {} — {}", mark, id, kind, sev, path, name);
                }
            });
        }
        TaintAction::Check {
            path,
            agent_id: who,
            confidence,
        } => {
            let who = who.unwrap_or_else(|| agent_id.to_string());
            let url = format!(
                "{}/api/taint/check?path={}&agent_id={}&confidence={}",
                server,
                urlencoding(&path),
                urlencoding(&who),
                confidence,
            );
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "taint check failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let can_write = v["can_write"].as_bool().unwrap_or(false);
                let isolated = v["isolated"].as_bool().unwrap_or(false);
                let req = v["required_confidence"].as_f64().unwrap_or(0.0);
                println!(
                    "can_write: {}{}",
                    can_write,
                    if isolated { " (isolated)" } else { "" }
                );
                println!("required_confidence: {:.2}", req);
                if let Some(eff) = v["effect"].as_str() {
                    println!("effect: {}", eff);
                }
                if let Some(id) = v["matching_taint_id"].as_str() {
                    println!("matching_taint_id: {}", id);
                }
                let warnings = v["warnings"].as_array().cloned().unwrap_or_default();
                if !warnings.is_empty() {
                    println!("warnings:");
                    for w in &warnings {
                        if let Some(s) = w.as_str() {
                            println!("  - {}", s);
                        }
                    }
                }
            });
        }
        TaintAction::Apply {
            path,
            name,
            kind,
            effect,
            severity,
            reason,
            authorized,
        } => {
            let mut body = serde_json::json!({
                "path": path,
                "name": name,
                "kind": kind,
                "effect": effect.unwrap_or_else(|| "warn".to_string()),
                "severity": severity,
                "reason": reason,
                "agent_id": agent_id,
                "ref_name": branch,
            });
            if !authorized.is_empty() {
                body["authorized_agents"] = serde_json::json!(authorized);
            }
            let url = format!("{}/api/taint", server);
            let resp = match client.post(&url).json(&body).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "taint apply failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let id = v["taint_id"].as_str().unwrap_or("");
                let path = v["path"].as_str().unwrap_or("");
                println!("Applied {} on {}", id, path);
            });
        }
        TaintAction::Remove { taint_id, reason } => {
            let body = serde_json::json!({
                "reason": reason,
                "agent_id": agent_id,
                "ref_name": branch,
            });
            let url = format!("{}/api/taint/{}", server, urlencoding(&taint_id));
            let resp = match client.delete(&url).json(&body).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "taint remove failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let at = v["resolved_at"].as_str().unwrap_or("?");
                println!("Resolved {} at {}", taint_id, at);
            });
        }
    }
    Ok(())
}

// -- Db command implementation --

/// Dispatch for `ctx db <subcommand>`. `backup` calls the hub HTTP
/// endpoint; `restore` operates directly on the file system (the hub
/// must be stopped) so we can swap files atomically without trying
/// to coax the running hub into releasing its open fd.
async fn handle_db(
    action: DbAction,
    server: &str,
    branch: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        DbAction::Export { out } => {
            let url = format!("{}/api/export?ref={}", server, urlencoding(branch));
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "db export failed").await;
            }
            let parsed: Value = resp.json().await?;
            let pretty = serde_json::to_string_pretty(&parsed)?;
            let count = parsed["count"].as_u64().unwrap_or(0);
            match out {
                Some(path) => {
                    std::fs::write(&path, &pretty)?;
                    eprintln!("Exported {count} paths (branch {branch}) → {path}");
                }
                None => println!("{pretty}"),
            }
        }
        DbAction::Import { file } => {
            let content = std::fs::read_to_string(&file)?;
            let snapshot: Value = serde_json::from_str(&content)
                .map_err(|e| format!("{file} is not valid JSON: {e}"))?;
            // Accept either a full export ({paths:{…}}) or a bare {path:value} map.
            let paths = snapshot
                .get("paths")
                .cloned()
                .unwrap_or(snapshot);
            let body = serde_json::json!({ "ref": branch, "paths": paths });
            let resp = match client.post(format!("{server}/api/import")).json(&body).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "db import failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                println!(
                    "Imported {} paths onto branch {branch}.",
                    v["imported"].as_u64().unwrap_or(0)
                );
            });
        }
        DbAction::Backup { suffix } => {
            let mut body = serde_json::Map::new();
            if let Some(s) = suffix {
                body.insert("suffix".into(), Value::String(s));
            }
            let url = format!("{}/api/admin/backup", server);
            let resp = match client.post(&url).json(&Value::Object(body)).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "backup failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("?");
                println!("Snapshot written: {}", path);
            });
        }
        DbAction::Restore { snapshot, to, yes } => {
            // Refuse if hub is running. The lockfile is the truth
            // source — its presence-with-live-PID means a hub holds
            // an open fd we'd be invalidating.
            let lock_path = format!("{}.lock", to);
            if std::path::Path::new(&lock_path).exists() {
                if let Ok(body) = std::fs::read_to_string(&lock_path)
                    && let Some(pid_str) = body.split("\"pid\":").nth(1).map(|s| {
                        s.chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect::<String>()
                    })
                    && let Ok(pid) = pid_str.parse::<u32>()
                    && std::process::Command::new("kill")
                        .args(["-0", &pid.to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                {
                    eprintln!(
                        "ctx db restore: hub is running (pid {} holds {}); stop it first",
                        pid, lock_path
                    );
                    std::process::exit(EX_TEMPFAIL);
                }
            }

            // Sanity-check the snapshot exists and the destination
            // path is plausible. We don't open them as sqlite here —
            // the next hub start will surface schema problems and
            // can roll back via the .pre-restore-* sibling.
            if !std::path::Path::new(&snapshot).exists() {
                eprintln!("ctx db restore: snapshot not found: {}", snapshot);
                std::process::exit(EX_NOINPUT);
            }

            if !yes {
                eprintln!(
                    "About to restore {} → {}\n  current db will be moved to {}.pre-restore-<utc>\nProceed? [y/N] ",
                    snapshot, to, to
                );
                use std::io::BufRead;
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
                    eprintln!("aborted");
                    std::process::exit(0);
                }
            }

            // Generate a sibling backup name. Match the hub's own
            // VACUUM-INTO suffix style for symmetry.
            let suffix = {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("{}", secs) // simple unix timestamp; cheap and unambiguous
            };
            let preserved = format!("{}.pre-restore-{}", to, suffix);

            // Move current → preserved (only if it exists; first
            // restore against an empty dir is fine).
            if std::path::Path::new(&to).exists() {
                std::fs::rename(&to, &preserved)
                    .map_err(|e| format!("could not rename {} → {}: {}", to, preserved, e))?;
                eprintln!("preserved current db at {}", preserved);
            }

            // Copy snapshot → to. Use copy not rename so the snapshot
            // file stays put as a separate artifact.
            std::fs::copy(&snapshot, &to)
                .map_err(|e| format!("could not copy {} → {}: {}", snapshot, to, e))?;

            let result = serde_json::json!({
                "status": "ok",
                "restored_to": to,
                "from_snapshot": snapshot,
                "preserved_at": preserved,
            });
            emit(format, &result, |_| {
                println!("Restored {} from {}", to, snapshot);
                println!("  previous db preserved at {}", preserved);
                println!("  start the hub when ready");
            });
        }
    }
    Ok(())
}

/// `ctx docs` — the canonical-doc registry.
async fn handle_docs(
    action: DocsAction,
    server: &str,
    branch: &str,
    format: OutputFormat,
    client: reqwest::Client,
) -> Result<(), Box<dyn std::error::Error>> {
    let print_entry = |d: &Value| {
        let status = d["status"].as_str().unwrap_or("?");
        let scope = d["scope"].as_str().map(|s| format!(" — {s}")).unwrap_or_default();
        let answers = d["answers"]
            .as_str()
            .map(|s| format!("\n      answers: {s}"))
            .unwrap_or_default();
        println!("  {} [{}]{}{}", d["path"].as_str().unwrap_or(""), status, scope, answers);
    };
    match action {
        DocsAction::Add {
            path,
            status,
            scope,
            owner,
            answers,
            supersedes,
            verified_commit,
        } => {
            let body = serde_json::json!({
                "ref": branch, "path": path, "status": status, "scope": scope,
                "owner": owner, "answers": answers, "supersedes": supersedes,
                "last_verified_commit": verified_commit,
            });
            let resp = match client.post(format!("{server}/api/docs")).json(&body).send().await {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "docs add failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                println!("Registered {} [{}]", v["path"].as_str().unwrap_or(""), v["status"].as_str().unwrap_or(""));
            });
        }
        DocsAction::List => {
            let resp = match client
                .get(format!("{server}/api/docs?ref={}", urlencoding(branch)))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "docs list failed").await;
            }
            let parsed: Value = resp.json().await?;
            emit(format, &parsed, |v| {
                let empty = vec![];
                let arr = v.as_array().unwrap_or(&empty);
                if arr.is_empty() {
                    println!("No registered docs.");
                    return;
                }
                println!("Registered docs:");
                for d in arr {
                    print_entry(d);
                }
            });
        }
        DocsAction::Find { query } => {
            let resp = match client
                .get(format!("{server}/api/docs?ref={}", urlencoding(branch)))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => unreachable_exit(server, e),
            };
            if !resp.status().is_success() {
                http_error_exit(resp, "docs find failed").await;
            }
            let parsed: Value = resp.json().await?;
            let q = query.to_lowercase();
            let matches: Vec<Value> = parsed
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|d| {
                            ["path", "scope", "answers", "owner"].iter().any(|k| {
                                d[*k].as_str().map(|s| s.to_lowercase().contains(&q)).unwrap_or(false)
                            })
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            emit(format, &Value::Array(matches.clone()), |_| {
                if matches.is_empty() {
                    println!("No docs match '{query}'.");
                    return;
                }
                for d in &matches {
                    print_entry(d);
                }
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
            session: None,
            namespace: None,
            token: None,
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
            "proj-abc123",
        )
        .expect("merge should succeed on empty input");
        assert!(out.contains("[mcp_servers.ctxone]"));
        assert!(out.contains("command = \"/usr/local/bin/ctxone-hub\""));
        assert!(out.contains("--path"));
        assert!(out.contains("/home/user/.ctxone/memory.db"));
        // New: agent-id flag should be passed through
        assert!(out.contains("--agent-id"));
        assert!(out.contains("\"codex\""));
        // t-015: CTX_SESSION env baked in for savings persistence.
        assert!(out.contains("CTX_SESSION"));
        assert!(out.contains("proj-abc123"));
    }

    #[test]
    fn codex_merge_preserves_other_mcp_servers() {
        let existing = r#"
[mcp_servers.linear]
command = "wsl"
args = ["npx", "-y", "mcp-remote", "https://mcp.linear.app/sse"]
"#;
        let out = merge_codex_ctxone_toml(existing, "/bin/ctxone-hub", "/db", "codex", "proj-x")
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
        let out = merge_codex_ctxone_toml(existing, "/bin/ctxone-hub", "/db", "codex", "proj-x")
            .expect("merge should succeed");
        assert!(out.contains("project_trust_level = \"workspace-trusted\""));
        assert!(out.contains("some_other_setting = 42"));
        assert!(out.contains("[mcp_servers.figma]"));
        assert!(out.contains("[mcp_servers.ctxone]"));
    }

    #[test]
    fn codex_merge_is_idempotent() {
        // First merge
        let first = merge_codex_ctxone_toml("", "/bin/hub", "/db/main.db", "codex", "proj-x")
            .expect("first merge");
        // Second merge on the output of the first
        let second = merge_codex_ctxone_toml(&first, "/bin/hub", "/db/main.db", "codex", "proj-x")
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
        let out =
            merge_codex_ctxone_toml(existing, "/new/path/ctxone-hub", "/new/db", "codex", "proj-x")
                .expect("merge should succeed");
        assert!(out.contains("/new/path/ctxone-hub"));
        assert!(out.contains("/new/db"));
        assert!(!out.contains("/old/path/ctxone-hub"));
        assert!(!out.contains("/old/db"));
    }

    #[test]
    fn codex_merge_rejects_invalid_toml() {
        let broken = "this is { not valid toml }}";
        assert!(merge_codex_ctxone_toml(broken, "/bin/hub", "/db", "codex", "proj-x").is_err());
    }

    #[test]
    fn hub_health_url_derives_from_mcp_url() {
        assert_eq!(
            hub_health_url("http://localhost:3001/mcp").as_deref(),
            Some("http://localhost:3001/api/health")
        );
        assert_eq!(
            hub_health_url("https://hub.example:8443/mcp?namespace=p").as_deref(),
            Some("https://hub.example:8443/api/health")
        );
        assert_eq!(hub_health_url("not-a-url").as_deref(), None);
    }

    #[test]
    fn codex_http_merge_writes_url_entry() {
        let out = merge_codex_ctxone_toml_http("", "http://localhost:3001/mcp?namespace=p", None)
            .expect("http merge");
        assert!(out.contains("[mcp_servers.ctxone]"));
        assert!(out.contains("url = \"http://localhost:3001/mcp?namespace=p\""));
        assert!(!out.contains("command"));
        assert!(!out.contains("bearer_token_env_var"));
    }

    #[test]
    fn codex_http_merge_writes_bearer_env_var() {
        let out = merge_codex_ctxone_toml_http(
            "",
            "http://localhost:3001/mcp",
            Some("CTXONE_AUTH_TOKEN"),
        )
        .expect("http merge");
        assert!(out.contains("bearer_token_env_var = \"CTXONE_AUTH_TOKEN\""));
    }

    #[test]
    fn codex_http_merge_replaces_stale_stdio_entry() {
        // Switching an existing stdio Codex entry to http must drop command/args.
        let existing = r#"
[mcp_servers.ctxone]
command = "/old/ctxone-hub"
args = ["--path", "/old/db"]
"#;
        let out = merge_codex_ctxone_toml_http(existing, "http://localhost:3001/mcp", None)
            .expect("http merge");
        assert!(out.contains("url = \"http://localhost:3001/mcp\""));
        assert!(!out.contains("command"));
        assert!(!out.contains("/old/db"));
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
        let entry = mcp_server_entry(
            "claude-code",
            "Claude Code",
            McpTransport::Stdio,
            "http://localhost:3001/mcp",
            None,
            None,
            None,
        );
        let args = entry
            .get("args")
            .and_then(|v| v.as_array())
            .expect("args should be an array");
        let arg_strs: Vec<&str> = args.iter().filter_map(|v| v.as_str()).collect();
        assert!(arg_strs.contains(&"--agent-id"));
        assert!(arg_strs.contains(&"claude-code"));
    }

    #[test]
    fn mcp_server_entry_http_native_for_claude_code() {
        let entry = mcp_server_entry(
            "claude-code",
            "Claude Code",
            McpTransport::Http,
            "http://localhost:3001/mcp",
            Some("proj-ns"),
            None,
            None,
        );
        assert_eq!(entry.get("type").and_then(|v| v.as_str()), Some("http"));
        assert_eq!(
            entry.get("url").and_then(|v| v.as_str()),
            Some("http://localhost:3001/mcp?namespace=proj-ns")
        );
        assert!(entry.get("command").is_none(), "native http has no command");
        assert!(entry.get("headers").is_none(), "no token → no headers");
    }

    #[test]
    fn mcp_server_entry_http_native_embeds_literal_token() {
        let entry = mcp_server_entry(
            "claude-code",
            "Claude Code",
            McpTransport::Http,
            "http://localhost:3001/mcp",
            None,
            Some("s3cret"),
            None,
        );
        assert_eq!(
            entry["headers"]["Authorization"].as_str(),
            Some("Bearer s3cret")
        );
    }

    #[test]
    fn mcp_server_entry_http_native_uses_env_ref_when_no_literal() {
        let entry = mcp_server_entry(
            "cursor",
            "Cursor",
            McpTransport::Http,
            "http://localhost:3001/mcp",
            None,
            None,
            Some("CTXONE_AUTH_TOKEN"),
        );
        assert_eq!(
            entry["headers"]["Authorization"].as_str(),
            Some("Bearer ${CTXONE_AUTH_TOKEN}")
        );
    }

    #[test]
    fn mcp_server_entry_http_bridges_claude_desktop() {
        let entry = mcp_server_entry(
            "claude-desktop",
            "Claude Desktop",
            McpTransport::Http,
            "http://localhost:3001/mcp",
            Some("proj-ns"),
            None,
            None,
        );
        // Claude Desktop can't read {type:http,url}; must get the mcp-remote bridge.
        assert!(entry.get("type").is_none(), "bridge is a stdio command, not type:http");
        assert_eq!(entry.get("command").and_then(|v| v.as_str()), Some("npx"));
        let args: Vec<&str> = entry
            .get("args")
            .and_then(|v| v.as_array())
            .expect("args array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(args.contains(&"mcp-remote"));
        assert!(args.contains(&"http://localhost:3001/mcp?namespace=proj-ns"));
        assert!(args.contains(&"http-only"));
        assert!(!args.contains(&"--header"), "no token → no --header");
    }

    #[test]
    fn mcp_server_entry_bridge_adds_literal_header_token() {
        let entry = mcp_server_entry(
            "claude-desktop",
            "Claude Desktop",
            McpTransport::Http,
            "http://localhost:3001/mcp",
            None,
            Some("s3cret"),
            None,
        );
        let args: Vec<String> = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(args.iter().any(|a| a == "--header"));
        assert!(args.iter().any(|a| a == "Authorization: Bearer s3cret"));
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
