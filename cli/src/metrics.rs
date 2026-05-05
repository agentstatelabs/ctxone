//! Session token-usage metrics: parse Claude Code JSONL, compute costs,
//! detect units of work, and render tabular summaries.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Pricing (per million tokens) ─────────────────────────────────────────────

struct ModelPricing {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

fn pricing_for(model: &str) -> ModelPricing {
    if model.contains("claude-opus-4") {
        ModelPricing { input: 15.00, output: 75.00, cache_write: 18.75, cache_read: 1.50 }
    } else if model.contains("claude-sonnet-4")
        || model.contains("claude-sonnet-3-7")
        || model.contains("claude-3-7-sonnet")
        || model.contains("claude-sonnet-3-5")
        || model.contains("claude-3-5-sonnet")
    {
        ModelPricing { input: 3.00, output: 15.00, cache_write: 3.75, cache_read: 0.30 }
    } else if model.contains("claude-haiku-4")
        || model.contains("claude-3-5-haiku")
    {
        ModelPricing { input: 0.80, output: 4.00, cache_write: 1.00, cache_read: 0.08 }
    } else if model.contains("claude-haiku") {
        ModelPricing { input: 0.25, output: 1.25, cache_write: 0.30, cache_read: 0.03 }
    } else if model.contains("claude-opus") {
        ModelPricing { input: 15.00, output: 75.00, cache_write: 18.75, cache_read: 1.50 }
    } else if model.contains("claude") {
        ModelPricing { input: 3.00, output: 15.00, cache_write: 3.75, cache_read: 0.30 }
    } else {
        // Unknown / non-Anthropic
        ModelPricing { input: 3.00, output: 15.00, cache_write: 0.00, cache_read: 0.00 }
    }
}

// ── Data model ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Clone)]
pub struct TurnMetrics {
    pub model: String,
    pub timestamp: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub tool_calls: u64,
    pub web_searches: u64,
    pub web_fetches: u64,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct UnitOfWork {
    pub index: usize,
    pub start_ts: String,
    pub end_ts: String,
    pub duration_seconds: f64,
    pub turns: u64,
    pub tool_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_hit_rate: f64,
    pub model: String,
    pub cost_usd: f64,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct SessionMetrics {
    pub session_id: String,
    pub source_file: String,
    pub first_ts: String,
    pub last_ts: String,
    pub wall_seconds: f64,
    pub turns: u64,
    pub tool_calls: u64,
    pub web_searches: u64,
    pub web_fetches: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub models: HashMap<String, u64>,
    pub cache_hit_rate: f64,
    pub cost_usd: f64,
    pub cost_no_cache_usd: f64,
    pub cache_savings_usd: f64,
    pub units: Vec<UnitOfWork>,
    #[serde(skip)]
    pub turn_details: Vec<TurnMetrics>,
}

impl SessionMetrics {
    pub fn finalize(&mut self, gap_minutes: f64) {
        if !self.first_ts.is_empty() && !self.last_ts.is_empty() {
            if let (Ok(t0), Ok(t1)) = (
                self.first_ts.parse::<DateTime<Utc>>(),
                self.last_ts.parse::<DateTime<Utc>>(),
            ) {
                self.wall_seconds = (t1 - t0).num_seconds().max(0) as f64;
            }
        }

        let total_input = self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens;
        self.cache_hit_rate = if total_input > 0 {
            self.cache_read_tokens as f64 / total_input as f64
        } else {
            0.0
        };

        self.cost_usd = self.compute_cost(true);
        self.cost_no_cache_usd = self.compute_cost(false);
        self.cache_savings_usd = (self.cost_no_cache_usd - self.cost_usd).max(0.0);
        self.units = detect_units(&self.turn_details, gap_minutes);
    }

    fn dominant_model(&self) -> &str {
        self.models
            .iter()
            .max_by_key(|&(_, &v)| v)
            .map(|(k, _)| k.as_str())
            .unwrap_or("unknown")
    }

    fn compute_cost(&self, with_cache: bool) -> f64 {
        let p = pricing_for(self.dominant_model());
        let output_cost = self.output_tokens as f64 * p.output / 1_000_000.0;
        if with_cache {
            self.input_tokens as f64 * p.input / 1_000_000.0
                + output_cost
                + self.cache_read_tokens as f64 * p.cache_read / 1_000_000.0
                + self.cache_creation_tokens as f64 * p.cache_write / 1_000_000.0
        } else {
            let total_input =
                self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens;
            total_input as f64 * p.input / 1_000_000.0 + output_cost
        }
    }

    /// Accumulate another session into this aggregate.
    pub fn add(&mut self, other: &SessionMetrics) {
        self.turns += other.turns;
        self.tool_calls += other.tool_calls;
        self.web_searches += other.web_searches;
        self.web_fetches += other.web_fetches;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.wall_seconds += other.wall_seconds;
        self.cost_usd += other.cost_usd;
        self.cost_no_cache_usd += other.cost_no_cache_usd;
        self.cache_savings_usd += other.cache_savings_usd;
        for (model, count) in &other.models {
            *self.models.entry(model.clone()).or_insert(0) += count;
        }
        if self.first_ts.is_empty()
            || (!other.first_ts.is_empty() && other.first_ts < self.first_ts)
        {
            self.first_ts = other.first_ts.clone();
        }
        if other.last_ts > self.last_ts {
            self.last_ts = other.last_ts.clone();
        }
        let total_input = self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens;
        self.cache_hit_rate = if total_input > 0 {
            self.cache_read_tokens as f64 / total_input as f64
        } else {
            0.0
        };
    }
}

// ── Unit-of-work detection ─────────────────────────────────────────────────────

pub fn detect_units(turns: &[TurnMetrics], gap_minutes: f64) -> Vec<UnitOfWork> {
    if turns.is_empty() {
        return vec![];
    }
    let gap_secs = gap_minutes * 60.0;
    let mut units: Vec<UnitOfWork> = vec![];
    let mut bucket: Vec<&TurnMetrics> = vec![];

    for turn in turns {
        let split = bucket.last().map(|prev| {
            if let (Ok(t0), Ok(t1)) = (
                prev.timestamp.parse::<DateTime<Utc>>(),
                turn.timestamp.parse::<DateTime<Utc>>(),
            ) {
                (t1 - t0).num_seconds() as f64 > gap_secs
            } else {
                false
            }
        }).unwrap_or(false);

        if split {
            units.push(build_unit(units.len(), &bucket));
            bucket.clear();
        }
        bucket.push(turn);
    }
    if !bucket.is_empty() {
        units.push(build_unit(units.len(), &bucket));
    }
    units
}

fn build_unit(index: usize, turns: &[&TurnMetrics]) -> UnitOfWork {
    let start_ts = turns.first().map(|t| t.timestamp.clone()).unwrap_or_default();
    let end_ts = turns.last().map(|t| t.timestamp.clone()).unwrap_or_default();

    let duration_seconds = if let (Ok(t0), Ok(t1)) = (
        start_ts.parse::<DateTime<Utc>>(),
        end_ts.parse::<DateTime<Utc>>(),
    ) {
        (t1 - t0).num_seconds().max(0) as f64
    } else {
        0.0
    };

    let input: u64 = turns.iter().map(|t| t.input_tokens).sum();
    let output: u64 = turns.iter().map(|t| t.output_tokens).sum();
    let cache_read: u64 = turns.iter().map(|t| t.cache_read_tokens).sum();
    let cache_creation: u64 = turns.iter().map(|t| t.cache_creation_tokens).sum();
    let tool_calls: u64 = turns.iter().map(|t| t.tool_calls).sum();

    let total_input = input + cache_read + cache_creation;
    let cache_hit_rate = if total_input > 0 {
        cache_read as f64 / total_input as f64
    } else {
        0.0
    };

    let model = turns
        .iter()
        .filter(|t| !t.model.is_empty())
        .last()
        .map(|t| t.model.clone())
        .unwrap_or_default();

    let p = pricing_for(&model);
    let cost_usd = input as f64 * p.input / 1_000_000.0
        + output as f64 * p.output / 1_000_000.0
        + cache_read as f64 * p.cache_read / 1_000_000.0
        + cache_creation as f64 * p.cache_write / 1_000_000.0;

    UnitOfWork {
        index,
        start_ts,
        end_ts,
        duration_seconds,
        turns: turns.len() as u64,
        tool_calls,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        cache_hit_rate,
        model,
        cost_usd,
    }
}

// ── Parsing ────────────────────────────────────────────────────────────────────

pub fn parse_session_metrics(path: &Path, gap_minutes: f64) -> SessionMetrics {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("warn: could not read {}: {}", path.display(), e);
            return SessionMetrics::default();
        }
    };

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut sm = SessionMetrics {
        session_id,
        source_file: path.to_string_lossy().to_string(),
        ..Default::default()
    };

    let mut seen_uuids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if entry.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }

        if let Some(uuid) = entry.get("uuid").and_then(|v| v.as_str()) {
            if !seen_uuids.insert(uuid.to_string()) {
                continue;
            }
        }

        let msg = match entry.get("message") {
            Some(m) => m,
            None => continue,
        };

        let usage = match msg.get("usage") {
            Some(u) => u,
            None => continue,
        };

        let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_creation = usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if input == 0 && output == 0 && cache_read == 0 && cache_creation == 0 {
            continue;
        }

        let model = normalize_model(
            msg.get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
        );

        let timestamp = entry
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut tool_calls = 0u64;
        let mut web_searches = 0u64;
        let mut web_fetches = 0u64;
        if let Some(blocks) = msg.get("content").and_then(|v| v.as_array()) {
            for block in blocks {
                if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                    tool_calls += 1;
                    match block.get("name").and_then(|v| v.as_str()) {
                        Some("WebSearch") | Some("web_search") => web_searches += 1,
                        Some("WebFetch") | Some("web_fetch") => web_fetches += 1,
                        _ => {}
                    }
                }
            }
        }

        if (sm.first_ts.is_empty() || timestamp < sm.first_ts) && !timestamp.is_empty() {
            sm.first_ts = timestamp.clone();
        }
        if timestamp > sm.last_ts {
            sm.last_ts = timestamp.clone();
        }

        sm.input_tokens += input;
        sm.output_tokens += output;
        sm.cache_read_tokens += cache_read;
        sm.cache_creation_tokens += cache_creation;
        sm.tool_calls += tool_calls;
        sm.web_searches += web_searches;
        sm.web_fetches += web_fetches;
        sm.turns += 1;
        *sm.models.entry(model.clone()).or_insert(0) += 1;

        sm.turn_details.push(TurnMetrics {
            model,
            timestamp,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            tool_calls,
            web_searches,
            web_fetches,
        });
    }

    sm.finalize(gap_minutes);
    sm
}

fn normalize_model(model: &str) -> String {
    // Strip 8-digit date suffix: "claude-haiku-4-5-20251001" -> "claude-haiku-4-5"
    if let Some(pos) = model.rfind('-') {
        let suffix = &model[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            return model[..pos].to_string();
        }
    }
    model.to_string()
}

// ── File discovery ─────────────────────────────────────────────────────────────

/// Find all Claude Code session files for a given project directory.
pub fn find_session_files(project_dir: &Path) -> Vec<PathBuf> {
    crate::ingest::find_session_files(project_dir)
}

/// List every project in ~/.claude/projects/ and return (label, files) pairs.
pub fn all_project_sessions() -> Vec<(String, Vec<PathBuf>)> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let projects_dir = home.join(".claude").join("projects");

    let mut result = vec![];
    let Ok(entries) = std::fs::read_dir(&projects_dir) else {
        return result;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let hash = path.file_name().unwrap_or_default().to_string_lossy();
        // Reverse the hash: replace leading '-' then split on '-' to recover path
        // Hash is: project_path.replace('/', '-'), so starts with '-' for absolute paths.
        let label = if hash.starts_with('-') {
            // Take last two path components for a readable label
            let parts: Vec<&str> = hash.trim_start_matches('-').split('-').collect();
            if parts.len() >= 2 {
                format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
            } else {
                hash.to_string()
            }
        } else {
            hash.to_string()
        };

        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
            .collect();
        files.sort_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());

        if !files.is_empty() {
            result.push((label, files));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

// ── Formatting helpers ─────────────────────────────────────────────────────────

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn fmt_duration(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}

pub fn fmt_ts_short(ts: &str) -> String {
    // "2025-01-02T15:04:05.000Z" -> "2025-01-02 15:04"
    ts.get(..16)
        .map(|s| s.replace('T', " "))
        .unwrap_or_else(|| ts.to_string())
}

/// Render a single-session or aggregate metrics table to stdout.
pub fn render_metrics(sm: &SessionMetrics, label: &str, gap: f64, verbose: bool) {
    let bar = "─".repeat(62);
    println!("\n{}", bar);
    println!("  Session Metrics — {}", label);
    println!("{}", bar);

    let model_str = {
        let mut v: Vec<(&String, &u64)> = sm.models.iter().collect();
        v.sort_by_key(|&(_, &c)| std::cmp::Reverse(c));
        v.iter()
            .take(3)
            .map(|(m, _)| m.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    println!("  Turns            {:>8}  (model: {})", sm.turns, model_str);
    println!(
        "  Tool calls       {:>8}  (web search: {}, fetch: {})",
        sm.tool_calls, sm.web_searches, sm.web_fetches
    );
    if sm.wall_seconds > 0.0 {
        println!("  Wall clock       {:>8}", fmt_duration(sm.wall_seconds));
    }

    println!();
    println!("  Token Usage");
    println!("    Input          {:>10}", fmt_tokens(sm.input_tokens));
    println!("    Output         {:>10}", fmt_tokens(sm.output_tokens));
    println!(
        "    Cache read     {:>10}  ({:.1}% hit rate)",
        fmt_tokens(sm.cache_read_tokens),
        sm.cache_hit_rate * 100.0
    );
    println!("    Cache write    {:>10}", fmt_tokens(sm.cache_creation_tokens));
    let total_in = sm.input_tokens + sm.cache_read_tokens + sm.cache_creation_tokens;
    println!("    Total input    {:>10}", fmt_tokens(total_in));

    println!();
    println!("  Cost");
    println!("    With cache     {:>10}", format!("${:.4}", sm.cost_usd));
    println!("    Without cache  {:>10}", format!("${:.4}", sm.cost_no_cache_usd));
    println!(
        "    Savings        {:>10}  ({:.1}%)",
        format!("${:.4}", sm.cache_savings_usd),
        if sm.cost_no_cache_usd > 0.0 {
            sm.cache_savings_usd / sm.cost_no_cache_usd * 100.0
        } else {
            0.0
        }
    );

    if !sm.units.is_empty() {
        println!();
        println!("  Units of Work  (gap = {:.0} min)", gap);
        for u in &sm.units {
            let start = fmt_ts_short(&u.start_ts);
            println!(
                "    #{:<2}  {}  {:>6}  {:>3} turns  {:>8} in  {:>7} out  {:>5.1}% cache  ${:.4}",
                u.index + 1,
                start,
                fmt_duration(u.duration_seconds),
                u.turns,
                fmt_tokens(u.input_tokens),
                fmt_tokens(u.output_tokens),
                u.cache_hit_rate * 100.0,
                u.cost_usd,
            );
        }
    }

    if verbose && !sm.turn_details.is_empty() {
        println!();
        println!("  Turn Detail");
        println!(
            "    {:4}  {:16}  {:>8}  {:>7}  {:>8}  {:>8}  {}",
            "#", "Timestamp", "Input", "Output", "CacheRd", "CacheWr", "Model"
        );
        for (i, t) in sm.turn_details.iter().enumerate() {
            println!(
                "    {:4}  {:16}  {:>8}  {:>7}  {:>8}  {:>8}  {}",
                i + 1,
                fmt_ts_short(&t.timestamp),
                fmt_tokens(t.input_tokens),
                fmt_tokens(t.output_tokens),
                fmt_tokens(t.cache_read_tokens),
                fmt_tokens(t.cache_creation_tokens),
                t.model,
            );
        }
    }

    println!("{}", bar);
}

/// Render a compact list row for `--list` mode.
pub fn render_list_row(sm: &SessionMetrics) {
    let ts = fmt_ts_short(&sm.first_ts);
    println!(
        "  {}  {:16}  {:>3} turns  {:>8} in  {:>7} out  {:>5.1}% cache  ${:.4}",
        &sm.session_id[..8.min(sm.session_id.len())],
        ts,
        sm.turns,
        fmt_tokens(sm.input_tokens),
        fmt_tokens(sm.output_tokens),
        sm.cache_hit_rate * 100.0,
        sm.cost_usd,
    );
}
