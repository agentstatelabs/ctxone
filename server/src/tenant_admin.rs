//! `ctxone-hub tenant`: a multi-tenant Hub's tenants, their tokens in the
//! tenants file, and deleting a tenant's data (docs/design/MULTI_TENANT_HUB.md,
//! "Administration").
//!
//! - `add`, `rotate`, `disable` and `enable` edit the tenants file, and `list`
//!   reads it. A Hub re-reads the file every second, so a change needs no
//!   restart.
//! - `purge` deletes a disabled tenant's rows from the database, then its lines
//!   from the file.
//!
//! Edits keep the file's comments and other lines, hold `<file>.lock` so two
//! commands can't interleave, and replace the file with one rename, so a Hub
//! never reads half of it.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentstategraph_storage::{PostgresStorage, TenantPool};

use crate::hub_store::pg_error;
use crate::tenant::{self, DISABLED, Tenant, TokenHash};
use crate::tenant_cache::{OPEN_LOCK_CLASS, ROOT_TENANT, UPKEEP_EVERY};

/// What a new tenants file starts with.
const HEADER: &str = "\
# CTXone tenants: `<tenant> <sha256 of a token>` per line, or
# `disabled <tenant> <sha256 of a token>` for a token that's kept and refused.
# Edit with `ctxone-hub tenant`. A Hub re-reads this file every second.
";

/// A new token: `ctx_` and 32 random bytes in hex.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system's random number source failed");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("ctx_{hex}")
}

/// A tenants file, read for editing.
pub struct Roster {
    path: PathBuf,
    lines: Vec<Line>,
    /// The text read, so that saving an unchanged roster writes nothing.
    read: String,
    /// Whether there was a file to read.
    existed: bool,
}

enum Line {
    /// A blank line or a comment, kept as it was.
    Text(String),
    Token {
        tenant: Tenant,
        hash: TokenHash,
        disabled: bool,
    },
}

/// A tenant as its file lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub tenant: Tenant,
    /// Tokens a Hub accepts.
    pub tokens: usize,
    /// Tokens kept and refused.
    pub disabled: usize,
}

impl Roster {
    /// Read the tenants file at `path`. A missing file is an empty roster; a
    /// file with a line a Hub can't use is refused, so nothing is saved over
    /// it until it's fixed.
    pub fn read(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let (text, existed) = match std::fs::read_to_string(&path) {
            Ok(text) => (text, true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (HEADER.to_string(), false),
            Err(e) => return Err(format!("can't read {}: {e}", path.display())),
        };
        let (_, problems) = tenant::parse(&text);
        if !problems.is_empty() {
            return Err(format!(
                "{}: fix these lines first: {}",
                path.display(),
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let lines = text
            .lines()
            .map(|raw| match tenant::parse_line(raw) {
                Some(Ok(entry)) => Line::Token {
                    tenant: entry.tenant,
                    hash: entry.hash,
                    disabled: entry.disabled,
                },
                // Checked above: a line is a token, a blank or a comment.
                _ => Line::Text(raw.to_string()),
            })
            .collect();
        Ok(Self {
            path,
            lines,
            read: text,
            existed,
        })
    }

    /// Read the tenants file at `path`, change it with `change`, and save it
    /// if `change` succeeds, holding `<path>.lock` throughout so that another
    /// command's edit can't be lost.
    pub fn edit<T>(
        path: impl Into<PathBuf>,
        change: impl FnOnce(&mut Roster) -> Result<T, String>,
    ) -> Result<T, String> {
        let path = path.into();
        let _lock = lock(&path)?;
        let mut roster = Self::read(path)?;
        let result = change(&mut roster)?;
        roster.save()?;
        Ok(result)
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file existed when it was read.
    pub fn existed(&self) -> bool {
        self.existed
    }

    /// The tenants, sorted, with their tokens.
    pub fn tenants(&self) -> Vec<Listed> {
        let mut listed: Vec<Listed> = Vec::new();
        for (tenant, disabled) in self.tokens() {
            let i = match listed.iter().position(|l| l.tenant == *tenant) {
                Some(i) => i,
                None => {
                    listed.push(Listed {
                        tenant: tenant.clone(),
                        tokens: 0,
                        disabled: 0,
                    });
                    listed.len() - 1
                }
            };
            if disabled {
                listed[i].disabled += 1;
            } else {
                listed[i].tokens += 1;
            }
        }
        listed.sort_by(|a, b| a.tenant.cmp(&b.tenant));
        listed
    }

    /// `tenant`'s line count, accepted tokens first: `None` when the file
    /// doesn't list it.
    pub fn find(&self, tenant: &Tenant) -> Option<Listed> {
        self.tenants().into_iter().find(|l| l.tenant == *tenant)
    }

    /// Whether `tenant` holds a token a Hub accepts.
    pub fn admits(&self, tenant: &Tenant) -> bool {
        self.find(tenant).is_some_and(|l| l.tokens > 0)
    }

    /// Add `tenant` with a new token, and return the token. Refuses a tenant
    /// the file already lists.
    pub fn add(&mut self, tenant: &Tenant) -> Result<String, String> {
        if self.find(tenant).is_some() {
            return Err(format!(
                "{} already has tenant {tenant}; `ctxone-hub tenant rotate {tenant}` replaces its tokens",
                self.path.display()
            ));
        }
        let token = new_token();
        self.lines.push(Line::Token {
            tenant: tenant.clone(),
            hash: TokenHash::of(&token),
            disabled: false,
        });
        Ok(token)
    }

    /// Replace every token of `tenant` with a new one, and return it and how
    /// many were replaced. Refuses a tenant that's disabled, since its new
    /// token wouldn't work.
    pub fn rotate(&mut self, tenant: &Tenant) -> Result<(String, usize), String> {
        let listed = self.require(tenant)?;
        if listed.tokens == 0 {
            return Err(format!(
                "tenant {tenant} is disabled; `ctxone-hub tenant enable {tenant}` first"
            ));
        }
        let token = new_token();
        // The new line takes the first old one's place.
        let first = self
            .lines
            .iter()
            .position(|l| matches!(l, Line::Token { tenant: t, .. } if t == tenant))
            .expect("require found the tenant");
        self.lines[first] = Line::Token {
            tenant: tenant.clone(),
            hash: TokenHash::of(&token),
            disabled: false,
        };
        let mut i = 0;
        self.lines.retain(|l| {
            let keep = i <= first || !matches!(l, Line::Token { tenant: t, .. } if t == tenant);
            i += 1;
            keep
        });
        Ok((token, listed.tokens + listed.disabled))
    }

    /// Disable or enable every token of `tenant`, and return how many changed.
    pub fn set_disabled(&mut self, tenant: &Tenant, disabled: bool) -> Result<usize, String> {
        self.require(tenant)?;
        let mut changed = 0;
        for line in &mut self.lines {
            if let Line::Token {
                tenant: t,
                disabled: d,
                ..
            } = line
                && t == tenant
                && *d != disabled
            {
                *d = disabled;
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Remove every line of `tenant`, and return how many there were.
    pub fn remove(&mut self, tenant: &Tenant) -> usize {
        let before = self.lines.len();
        self.lines
            .retain(|l| !matches!(l, Line::Token { tenant: t, .. } if t == tenant));
        before - self.lines.len()
    }

    fn require(&self, tenant: &Tenant) -> Result<Listed, String> {
        if !self.existed {
            return Err(no_file(&self.path));
        }
        self.find(tenant)
            .ok_or_else(|| format!("{} has no tenant {tenant}", self.path.display()))
    }

    fn tokens(&self) -> impl Iterator<Item = (&Tenant, bool)> {
        self.lines.iter().filter_map(|l| match l {
            Line::Token {
                tenant, disabled, ..
            } => Some((tenant, *disabled)),
            Line::Text(_) => None,
        })
    }

    fn render(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            match line {
                Line::Text(raw) => text.push_str(raw),
                Line::Token {
                    tenant,
                    hash,
                    disabled,
                } => {
                    if *disabled {
                        text.push_str(DISABLED);
                        text.push(' ');
                    }
                    text.push_str(&format!("{tenant} {}", hash.to_hex()));
                }
            }
            text.push('\n');
        }
        text
    }

    /// Write the roster back. A new file is readable by its owner only; an
    /// existing one keeps its permissions.
    fn save(&self) -> Result<(), String> {
        let text = self.render();
        if text == self.read {
            return Ok(());
        }
        // Never save what a Hub would refuse.
        let (_, problems) = tenant::parse(&text);
        assert!(
            problems.is_empty(),
            "a roster rendered a bad line: {problems:?}"
        );
        replace(&self.path, &text).map_err(|e| format!("can't write {}: {e}", self.path.display()))
    }
}

fn no_file(path: &Path) -> String {
    format!(
        "there's no tenants file at {}; `ctxone-hub tenant add` makes one",
        path.display()
    )
}

/// Hold `<path>.lock` until the returned file is dropped.
fn lock(path: &Path) -> Result<File, String> {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    let lock_path = PathBuf::from(name);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("can't open {}: {e}", lock_path.display()))?;
    file.lock()
        .map_err(|e| format!("can't lock {}: {e}", lock_path.display()))?;
    Ok(file)
}

/// Replace the file at `path` with `text` in one rename, so a reader sees the
/// old file or the new one, never part of either. Writes through a symlink to
/// its target.
fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    let target = match std::fs::canonicalize(path) {
        Ok(target) => target,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(e),
    };
    let existing = std::fs::metadata(&target).ok().map(|m| m.permissions());
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    // Left by a run that died here; the lock keeps live runs out.
    let _ = std::fs::remove_file(&tmp);
    let written = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(text.as_bytes())?;
        if let Some(permissions) = existing {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        std::fs::rename(&tmp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// The tables that hold a tenant's rows, in the order a purge deletes from
/// them: AgentStateGraph's, then the Hub's, with a project's paths before the
/// project they refer to. A purge refuses a database with a `tenant_id` table
/// that isn't here, rather than leave rows behind.
pub const TENANT_TABLES: &[&str] = &[
    // AgentStateGraph (agentstategraph-storage's Postgres schema).
    "objects",
    "commits",
    "commit_traces",
    "namespaces",
    "refs",
    "epochs",
    "sessions",
    "taints",
    "reminders",
    "leaf_rows",
    "leaf_index_refs",
    "gc_pending_roots",
    "asg_history_cursor",
    "asg_history_commit_rollup",
    "asg_history_milestone",
    // The Hub's own (crate::hub_store).
    "ctxone_project_paths",
    "ctxone_projects",
    "ctxone_sessions",
];

/// How long a purge waits after it's confirmed, so that every Hub that had the
/// tenant open has seen that no token of its works (Hubs re-read the tenants
/// file every second) and closed it at an upkeep pass (every 30 seconds),
/// writing its statistics for the last time. Rows deleted before that could
/// come back.
pub const PURGE_WAIT: Duration = Duration::from_secs(2 * UPKEEP_EVERY.as_secs() + 1);

/// The tables of the database's current schema that have a `tenant_id`
/// column. Base tables only: a view's rows are some table's.
const TENANT_ID_TABLES: &str = "
    SELECT c.table_name::text
    FROM information_schema.columns c
    JOIN information_schema.tables t
      ON t.table_schema = c.table_schema AND t.table_name = c.table_name
    WHERE c.table_schema = current_schema()
      AND c.column_name = 'tenant_id'
      AND t.table_type = 'BASE TABLE'
    ORDER BY 1";

/// Delete every row of `tenant` from `tables` (the Hub passes
/// [`TENANT_TABLES`]) in one transaction, in their order, and return how many
/// each table held, for the tables that held any. Holds the tenant's open lock
/// meanwhile, so no Hub opens it halfway through. Refuses, deleting nothing,
/// when the database has a `tenant_id` table that isn't in `tables`. A table
/// in `tables` that the database lacks is skipped. `pool` may be any tenant's:
/// the connection works for `tenant`, so row-level security lets it at
/// `tenant`'s rows, and only those.
pub fn purge_rows(
    pool: &TenantPool,
    tenant: &Tenant,
    tables: &[&'static str],
) -> Result<Vec<(&'static str, u64)>, String> {
    let pool = pool.for_tenant(tenant.as_str());
    let name = tenant.as_str().to_owned();
    let tables = tables.to_vec();
    PostgresStorage::run(async move {
        let mut client = pool.get().await.map_err(|e| e.to_string())?;
        let tx = client.transaction().await.map_err(|e| pg_error(&e))?;
        tx.batch_execute("SET LOCAL lock_timeout = '30s'")
            .await
            .map_err(|e| pg_error(&e))?;
        tx.execute(
            "SELECT pg_advisory_xact_lock($1, hashtext($2))",
            &[&OPEN_LOCK_CLASS, &name],
        )
        .await
        .map_err(|e| format!("lock the tenant: {}", pg_error(&e)))?;
        let present: Vec<String> = tx
            .query(TENANT_ID_TABLES, &[])
            .await
            .map_err(|e| pg_error(&e))?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let unknown: Vec<&str> = present
            .iter()
            .map(String::as_str)
            .filter(|t| !tables.contains(t))
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "the database has tables with a tenant_id this ctxone-hub doesn't know: {}. \
                 Purge with the ctxone-hub version that made them; nothing was deleted",
                unknown.join(", ")
            ));
        }
        let mut deleted = Vec::new();
        for table in tables {
            if !present.iter().any(|t| t == table) {
                continue;
            }
            let rows = tx
                .execute(
                    &format!("DELETE FROM \"{table}\" WHERE tenant_id = $1"),
                    &[&name],
                )
                .await
                .map_err(|e| format!("delete from {table}: {}", pg_error(&e)))?;
            if rows > 0 {
                deleted.push((table, rows));
            }
        }
        tx.commit().await.map_err(|e| pg_error(&e))?;
        Ok(deleted)
    })
}

/// Where a command's output goes, and how purge asks before deleting.
pub struct Console<'a> {
    /// A new token, or the tenant list: what a script would read.
    pub out: &'a mut dyn Write,
    /// What happened, for the person running the command.
    pub err: &'a mut dyn Write,
    /// Reads a line of the person's answer. `None` when nobody can answer,
    /// because stdin isn't a terminal.
    pub ask: Option<&'a mut dyn FnMut() -> std::io::Result<String>>,
}

/// Exit codes: success, a failure, and a command used wrongly (EX_USAGE).
const OK: i32 = 0;
const FAILED: i32 = 1;
const USAGE: i32 = 64;

const HELP: &str = "\
Usage: ctxone-hub tenant <command> [<name>] [options]

Manage a multi-tenant Hub's tenants: their tokens, kept in the tenants file,
and their data.

Commands:
  add <name>       Create a tenant and print its token, once. Only the token's
                   hash is kept.
  list             The tenants and their tokens
  rotate <name>    Replace a tenant's tokens with a new one and print it
  disable <name>   Refuse a tenant's tokens. Its data stays.
  enable <name>    Accept a disabled tenant's tokens again
  purge <name>     Delete all of a disabled tenant's data, then its tokens.
                   Asks first, and can't be undone.

Options:
  --tenants-file <PATH>  The tenants file (env: CTXONE_TENANTS_FILE)
  --database-url <URL>   purge: the Hub's Postgres database (env: DATABASE_URL)
  --yes                  purge: don't ask

A Hub re-reads its tenants file every second, so changes need no restart.
Tenant names are 1 to 63 of a-z, 0-9 and -.
";

/// Run `ctxone-hub tenant` from the binary: stdout, stderr, and a question on
/// the terminal when stdin is one. Returns the exit code.
pub fn main(args: &[String]) -> i32 {
    use std::io::IsTerminal;
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut read_line = || {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line)
    };
    let ask: Option<&mut dyn FnMut() -> std::io::Result<String>> = if std::io::stdin().is_terminal()
    {
        Some(&mut read_line)
    } else {
        None
    };
    let mut console = Console {
        out: &mut out,
        err: &mut err,
        ask,
    };
    run(args, &mut console, PURGE_WAIT)
}

/// Run `ctxone-hub tenant <args>`, waiting `purge_wait` before a purge
/// deletes (see [`PURGE_WAIT`]). Returns the exit code.
pub fn run(args: &[String], console: &mut Console, purge_wait: Duration) -> i32 {
    match run_command(args, console, purge_wait) {
        Ok(()) => OK,
        Err(Failure::Usage(why)) => {
            let _ = writeln!(console.err, "ctxone-hub tenant: {why}\n\n{HELP}");
            USAGE
        }
        Err(Failure::Failed(why)) => {
            let _ = writeln!(console.err, "ctxone-hub tenant: {why}");
            FAILED
        }
    }
}

enum Failure {
    Usage(String),
    Failed(String),
}

impl From<String> for Failure {
    fn from(why: String) -> Self {
        Failure::Failed(why)
    }
}

struct Args {
    command: String,
    name: Option<String>,
    tenants_file: Option<String>,
    database_url: Option<String>,
    yes: bool,
}

fn parse_args(args: &[String]) -> Result<Option<Args>, Failure> {
    let env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let mut parsed = Args {
        command: String::new(),
        name: None,
        tenants_file: env("CTXONE_TENANTS_FILE"),
        database_url: env("DATABASE_URL"),
        yes: false,
    };
    let mut positional = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || {
            rest.next()
                .filter(|v| !v.is_empty())
                .cloned()
                .ok_or_else(|| Failure::Usage(format!("{arg} needs a value")))
        };
        match arg.as_str() {
            "-h" | "--help" | "help" => return Ok(None),
            "--tenants-file" => parsed.tenants_file = Some(value()?),
            "--database-url" => parsed.database_url = Some(value()?),
            "--yes" | "-y" => parsed.yes = true,
            flag if flag.starts_with('-') => {
                return Err(Failure::Usage(format!("unknown option {flag}")));
            }
            word => positional.push(word.to_string()),
        }
    }
    let mut positional = positional.into_iter();
    let Some(command) = positional.next() else {
        return Ok(None);
    };
    parsed.command = command;
    parsed.name = positional.next();
    if let Some(extra) = positional.next() {
        return Err(Failure::Usage(format!("unexpected {extra}")));
    }
    Ok(Some(parsed))
}

fn run_command(
    args: &[String],
    console: &mut Console,
    purge_wait: Duration,
) -> Result<(), Failure> {
    let Some(args) = parse_args(args)? else {
        let _ = write!(console.out, "{HELP}");
        return Ok(());
    };
    let named = matches!(
        args.command.as_str(),
        "add" | "rotate" | "disable" | "enable" | "purge"
    );
    if !named && args.command != "list" {
        return Err(Failure::Usage(format!("unknown command {}", args.command)));
    }
    let tenant = match (&args.name, named) {
        (Some(name), true) => Some(Tenant::new(name).map_err(|e| Failure::Usage(e.to_string()))?),
        (None, true) => {
            return Err(Failure::Usage(format!(
                "{} needs a tenant name",
                args.command
            )));
        }
        (Some(name), false) => return Err(Failure::Usage(format!("unexpected {name}"))),
        (None, false) => None,
    };
    let Some(path) = args.tenants_file.clone() else {
        return Err(Failure::Usage(
            "which tenants file? Pass --tenants-file or set CTXONE_TENANTS_FILE".into(),
        ));
    };
    let path = PathBuf::from(path);
    let shown = path.display().to_string();

    match (args.command.as_str(), tenant) {
        ("list", _) => {
            let roster = Roster::read(&path)?;
            if !roster.existed() {
                return Err(no_file(&path).into());
            }
            let tenants = roster.tenants();
            if tenants.is_empty() {
                say(console, format!("{shown} has no tenants."));
            }
            for l in tenants {
                let state = match (l.tokens, l.disabled) {
                    (0, d) => format!("disabled ({d} token{})", plural(d)),
                    (n, 0) => format!("{n} token{}", plural(n)),
                    (n, d) => format!("{n} token{}, {d} disabled", plural(n)),
                };
                let _ = writeln!(console.out, "{}\t{state}", l.tenant);
            }
        }
        ("add", Some(tenant)) => {
            let token = Roster::edit(&path, |r| r.add(&tenant))?;
            let _ = writeln!(console.out, "{token}");
            say(
                console,
                format!(
                    "Added tenant {tenant} to {shown}. Its token is above, shown this once: \
                     only its hash is kept. A Hub reading the file accepts it within a second."
                ),
            );
        }
        ("rotate", Some(tenant)) => {
            let (token, replaced) = Roster::edit(&path, |r| r.rotate(&tenant))?;
            let _ = writeln!(console.out, "{token}");
            say(
                console,
                format!(
                    "Replaced tenant {tenant}'s {replaced} token{} in {shown} with the one above, \
                     shown this once. The old ones stop working within a second.",
                    plural(replaced)
                ),
            );
        }
        ("disable", Some(tenant)) => {
            let changed = Roster::edit(&path, |r| r.set_disabled(&tenant, true))?;
            if changed == 0 {
                say(console, format!("Tenant {tenant} is already disabled."));
            } else {
                say(
                    console,
                    format!(
                        "Disabled tenant {tenant}'s {changed} token{} in {shown}. They're refused \
                         within a second, and a Hub closes the tenant within a minute. Its data \
                         stays: `ctxone-hub tenant enable {tenant}` lets it back in.",
                        plural(changed)
                    ),
                );
            }
        }
        ("enable", Some(tenant)) => {
            let changed = Roster::edit(&path, |r| r.set_disabled(&tenant, false))?;
            if changed == 0 {
                say(console, format!("Tenant {tenant} isn't disabled."));
            } else {
                say(
                    console,
                    format!(
                        "Enabled tenant {tenant}'s {changed} token{} in {shown}. They work again \
                         within a second.",
                        plural(changed)
                    ),
                );
            }
        }
        ("purge", Some(tenant)) => purge(&args, &tenant, &path, console, purge_wait)?,
        _ => unreachable!("commands are checked above"),
    }
    Ok(())
}

fn purge(
    args: &Args,
    tenant: &Tenant,
    path: &Path,
    console: &mut Console,
    wait: Duration,
) -> Result<(), Failure> {
    let shown = path.display().to_string();
    // Only a tenant this file lists, every token disabled: a mistyped
    // --tenants-file, or one no Hub reads, mustn't let a tenant in use go.
    let disabled_here = |roster: &Roster| -> Result<(), String> {
        let listed = roster.require(tenant).map_err(|why| {
            format!(
                "{why}. Purge deletes a tenant the Hub's tenants file lists as disabled; for \
                 data a removed tenant left behind, `add` it and `disable` it first"
            )
        })?;
        if listed.tokens > 0 {
            return Err(format!(
                "tenant {tenant} has {} token{} a Hub accepts. Disable it first: \
                 `ctxone-hub tenant disable {tenant}`",
                listed.tokens,
                plural(listed.tokens)
            ));
        }
        Ok(())
    };
    disabled_here(&Roster::read(path)?)?;
    let Some(url) = args.database_url.as_deref() else {
        return Err(Failure::Usage(
            "purge needs the Hub's database: pass --database-url or set DATABASE_URL".into(),
        ));
    };
    if !args.yes {
        let Some(ask) = console.ask.as_mut() else {
            return Err(format!(
                "purge deletes all of tenant {tenant}'s data and can't be undone. Run it from \
                 a terminal to confirm, or pass --yes"
            )
            .into());
        };
        let _ = write!(
            console.err,
            "This deletes all of tenant {tenant}'s data from the database, and its tokens from \
             {shown}. It can't be undone.\nType the tenant's name to go on: "
        );
        let _ = console.err.flush();
        let answer = ask().map_err(|e| format!("can't read the answer: {e}"))?;
        if answer.trim() != tenant.as_str() {
            return Err("that isn't the tenant's name; nothing was deleted"
                .to_string()
                .into());
        }
    }
    if !wait.is_zero() {
        let _ = writeln!(
            console.err,
            "Waiting {} seconds, so that every Hub that had tenant {tenant} open has closed it...",
            wait.as_secs()
        );
        std::thread::sleep(wait);
    }
    // Enabled again while we waited?
    disabled_here(&Roster::read(path)?).map_err(|why| format!("{why}. Nothing was deleted"))?;
    let storage = PostgresStorage::connect_tenant_blocking(url, ROOT_TENANT, 1)
        .map_err(|e| format!("connect to the database: {e}"))?;
    let deleted = purge_rows(&storage.tenant_pool(), tenant, TENANT_TABLES)?;
    let removed = Roster::edit(path, |r| Ok(r.remove(tenant)))?;
    let total: u64 = deleted.iter().map(|(_, n)| n).sum();
    let detail = if deleted.is_empty() {
        String::new()
    } else {
        format!(
            " ({})",
            deleted
                .iter()
                .map(|(t, n)| format!("{t} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let _ = writeln!(
        console.err,
        "Purged tenant {tenant}: deleted {total} row{}{detail}, and removed its {removed} \
         token{} from {shown}.",
        plural(total as usize),
        plural(removed)
    );
    Ok(())
}

/// Tell the person running the command what happened.
fn say(console: &mut Console, line: String) {
    let _ = writeln!(console.err, "{line}");
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(name: &str) -> Tenant {
        Tenant::new(name).unwrap()
    }

    fn file(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("tenants")
    }

    /// The tenant `token` reaches through the file at `path`, as a Hub reads it.
    fn reaches(path: &Path, token: &str) -> Option<String> {
        tenant::TenantsFile::open(path)
            .unwrap()
            .lookup(token)
            .map(|t| t.to_string())
    }

    #[test]
    fn tokens_are_new_each_time_and_marked() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(a.starts_with("ctx_"));
        assert_eq!(a.len(), 4 + 64);
        assert!(a[4..].bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn add_creates_the_file_and_its_token_reaches_the_tenant() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        let token = Roster::edit(&path, |r| r.add(&tenant("alice"))).unwrap();
        assert_eq!(reaches(&path, &token).as_deref(), Some("alice"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# CTXone tenants"), "{text}");
        assert!(!text.contains(&token), "the file holds the token itself");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn add_refuses_a_tenant_the_file_has() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        Roster::edit(&path, |r| r.add(&tenant("alice"))).unwrap();
        let err = Roster::edit(&path, |r| r.add(&tenant("alice"))).unwrap_err();
        assert!(err.contains("already has tenant alice"), "{err}");
    }

    #[test]
    fn rotate_replaces_every_token_and_keeps_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        let bob = TokenHash::of("bob-token").to_hex();
        let a1 = TokenHash::of("alice-1").to_hex();
        let a2 = TokenHash::of("alice-2").to_hex();
        std::fs::write(
            &path,
            format!("# ours\nalice {a1}\n\n  bob   {bob}  \n# alice's phone\nalice {a2}\n"),
        )
        .unwrap();
        let (token, replaced) = Roster::edit(&path, |r| r.rotate(&tenant("alice"))).unwrap();
        assert_eq!(replaced, 2);
        assert_eq!(reaches(&path, "alice-1"), None);
        assert_eq!(reaches(&path, "alice-2"), None);
        assert_eq!(reaches(&path, &token).as_deref(), Some("alice"));
        assert_eq!(reaches(&path, "bob-token").as_deref(), Some("bob"));
        let text = std::fs::read_to_string(&path).unwrap();
        let new = TokenHash::of(&token).to_hex();
        assert_eq!(
            text,
            format!("# ours\nalice {new}\n\nbob {bob}\n# alice's phone\n")
        );
    }

    #[test]
    fn disable_and_enable_keep_the_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        let token = Roster::edit(&path, |r| r.add(&tenant("alice"))).unwrap();
        let bob = Roster::edit(&path, |r| r.add(&tenant("bob"))).unwrap();

        assert_eq!(
            Roster::edit(&path, |r| r.set_disabled(&tenant("alice"), true)),
            Ok(1)
        );
        assert_eq!(reaches(&path, &token), None);
        assert_eq!(reaches(&path, &bob).as_deref(), Some("bob"));
        let roster = Roster::read(&path).unwrap();
        assert!(!roster.admits(&tenant("alice")));
        assert_eq!(
            roster.find(&tenant("alice")),
            Some(Listed {
                tenant: tenant("alice"),
                tokens: 0,
                disabled: 1
            })
        );
        // Again: nothing changes.
        assert_eq!(
            Roster::edit(&path, |r| r.set_disabled(&tenant("alice"), true)),
            Ok(0)
        );
        let err = Roster::edit(&path, |r| r.rotate(&tenant("alice"))).unwrap_err();
        assert!(err.contains("is disabled"), "{err}");

        assert_eq!(
            Roster::edit(&path, |r| r.set_disabled(&tenant("alice"), false)),
            Ok(1)
        );
        assert_eq!(reaches(&path, &token).as_deref(), Some("alice"));
    }

    #[test]
    fn commands_on_a_tenant_the_file_lacks_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        Roster::edit(&path, |r| r.add(&tenant("alice"))).unwrap();
        for result in [
            Roster::edit(&path, |r| r.rotate(&tenant("bob")).map(|_| ())),
            Roster::edit(&path, |r| r.set_disabled(&tenant("bob"), true).map(|_| ())),
        ] {
            let err = result.unwrap_err();
            assert!(err.contains("has no tenant bob"), "{err}");
        }
    }

    #[test]
    fn remove_drops_every_line_of_the_tenant() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        Roster::edit(&path, |r| r.add(&tenant("alice"))).unwrap();
        let bob = Roster::edit(&path, |r| r.add(&tenant("bob"))).unwrap();
        Roster::edit(&path, |r| r.set_disabled(&tenant("alice"), true)).unwrap();
        assert_eq!(
            Roster::edit(&path, |r| Ok(r.remove(&tenant("alice")))),
            Ok(1)
        );
        let roster = Roster::read(&path).unwrap();
        assert_eq!(roster.find(&tenant("alice")), None);
        assert_eq!(reaches(&path, &bob).as_deref(), Some("bob"));
    }

    #[test]
    fn a_file_with_a_bad_line_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        let text = "alice not-a-hash\n";
        std::fs::write(&path, text).unwrap();
        let err = Roster::edit(&path, |r| r.add(&tenant("bob"))).unwrap_err();
        assert!(err.contains("fix these lines first: line 1"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[cfg(unix)]
    #[test]
    fn saving_keeps_the_files_permissions_and_writes_through_a_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real-tenants");
        std::fs::write(&real, "").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.path().join("tenants");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let token = Roster::edit(&link, |r| r.add(&tenant("alice"))).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(reaches(&real, &token).as_deref(), Some("alice"));
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
        // Nothing left beside it but the lock.
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["real-tenants", "tenants", "tenants.lock"]);
    }

    #[test]
    fn edits_wait_for_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir);
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    Roster::edit(&path, |r| r.add(&tenant(&format!("t{i}")))).unwrap()
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(Roster::read(&path).unwrap().tenants().len(), 8);
    }
}
