//! `ctxone-hub db`: the Hub's Postgres database.
//!
//! `row-security on|off|status` switches row-level security for every table
//! that holds tenants' rows, AgentStateGraph's and the Hub's own, or says
//! where it stands. With it on, a statement sees and writes only the rows of
//! its connection's tenant: a backstop behind each query's own tenant filter.
//! It's off until switched on, per database, and Hubs using the database
//! needn't restart, since every connection they check out already names its
//! tenant.
//!
//! A Hub says at startup where it stands ([`log_row_security`]), and warns
//! when its database role skips it.

use agentstategraph_storage::{PostgresStorage, RowSecurity, StorageError};

use crate::hub_store::PgStore;
use crate::tenant_admin::Console;
use crate::tenant_cache::ROOT_TENANT;

/// Exit codes: success, a failure, and a command used wrongly (EX_USAGE).
const OK: i32 = 0;
const FAILED: i32 = 1;
const USAGE: i32 = 64;

const HELP: &str = "\
Usage: ctxone-hub db row-security <on|off|status> [options]

The Hub's Postgres database.

Commands:
  row-security on      Switch row-level security on for every table that holds
                       tenants' rows: a statement sees and writes only its own
                       tenant's rows, behind each query's tenant filter. Hubs
                       using the database needn't restart.
  row-security off     Switch it off
  row-security status  Whether it's on. Prints on, off or partial.

Options:
  --database-url <URL>  The Hub's Postgres database (env: DATABASE_URL)

Run it as the role that owns the tables. A superuser or a role with BYPASSRLS
skips row-level security, so connect Hubs as a role with neither. With it on,
pg_dump as the tables' owner fails: back up as a superuser or a role with
BYPASSRLS.
";

/// Run `ctxone-hub db` from the binary, on stdout and stderr. Returns the exit
/// code.
pub fn main(args: &[String]) -> i32 {
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut console = Console {
        out: &mut out,
        err: &mut err,
        ask: None,
    };
    run(args, &mut console)
}

/// Run `ctxone-hub db <args>`. Returns the exit code.
pub fn run(args: &[String], console: &mut Console) -> i32 {
    match run_command(args, console) {
        Ok(()) => OK,
        Err(Failure::Usage(why)) => {
            let _ = writeln!(console.err, "ctxone-hub db: {why}\n\n{HELP}");
            USAGE
        }
        Err(Failure::Failed(why)) => {
            let _ = writeln!(console.err, "ctxone-hub db: {why}");
            FAILED
        }
    }
}

enum Failure {
    Usage(String),
    Failed(String),
}

fn run_command(args: &[String], console: &mut Console) -> Result<(), Failure> {
    let mut database_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty());
    let mut positional = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                let _ = write!(console.out, "{HELP}");
                return Ok(());
            }
            "--database-url" => {
                database_url = Some(
                    rest.next()
                        .filter(|v| !v.is_empty())
                        .cloned()
                        .ok_or_else(|| Failure::Usage(format!("{arg} needs a value")))?,
                );
            }
            flag if flag.starts_with('-') => {
                return Err(Failure::Usage(format!("unknown option {flag}")));
            }
            word => positional.push(word),
        }
    }
    let change = match positional[..] {
        [] => {
            let _ = write!(console.out, "{HELP}");
            return Ok(());
        }
        ["row-security", "on"] => Some(true),
        ["row-security", "off"] => Some(false),
        ["row-security", "status"] => None,
        ["row-security"] => {
            return Err(Failure::Usage(
                "row-security needs on, off or status".into(),
            ));
        }
        ["row-security", word, ..] if !matches!(word, "on" | "off" | "status") => {
            return Err(Failure::Usage(format!(
                "unknown row-security command {word}"
            )));
        }
        ["row-security", _, extra, ..] => {
            return Err(Failure::Usage(format!("unexpected {extra}")));
        }
        [command, ..] => return Err(Failure::Usage(format!("unknown command {command}"))),
    };
    let Some(url) = database_url else {
        return Err(Failure::Usage(
            "which database? Pass --database-url or set DATABASE_URL".into(),
        ));
    };

    let storage = PostgresStorage::connect_tenant_blocking(&url, ROOT_TENANT, 1)
        .map_err(|e| Failure::Failed(format!("connect to the database: {e}")))?;
    // The Hub's own tables get the tenant policy where they're set up, so set
    // them up first: switching covers every table with the policy.
    PgStore::new(&storage).map_err(Failure::Failed)?;
    let status = match change {
        Some(on) => storage.set_row_security(on),
        None => storage.row_security(),
    }
    .map_err(|e| Failure::Failed(e.to_string()))?;

    let _ = writeln!(console.out, "{}", state(&status));
    let tables = |names: &[String]| names.join(", ");
    let said = match (state(&status), change) {
        ("on", Some(true)) => format!(
            "Row-level security is on for all {} tables that hold tenants' rows. Hubs using \
             the database are held to it from their next query.",
            status.on.len()
        ),
        ("on", _) => format!(
            "Row-level security is on for all {} tables that hold tenants' rows.",
            status.on.len()
        ),
        ("off", Some(false)) => format!(
            "Row-level security is off for all {} tables that hold tenants' rows.",
            status.off.len()
        ),
        ("off", _) => format!(
            "Row-level security is off for all {} tables that hold tenants' rows: each query's \
             own tenant filter is what keeps tenants apart. `ctxone-hub db row-security on` \
             switches it on.",
            status.off.len()
        ),
        _ => format!(
            "Row-level security is on for {} tables and off for {}: {}. \
             `ctxone-hub db row-security on` switches it on for the rest.",
            status.on.len(),
            status.off.len(),
            tables(&status.off)
        ),
    };
    say(console, said);
    if !status.unguarded.is_empty() {
        say(
            console,
            format!(
                "These tables have a tenant_id column and no tenant policy, so row-level \
                 security doesn't cover them: {}.",
                tables(&status.unguarded)
            ),
        );
    }
    if status.role_bypasses {
        say(
            console,
            "This command's database role is a superuser or has BYPASSRLS, so it skips \
             row-level security, and so would a Hub connecting as it. Connect Hubs as a role \
             with neither."
                .to_string(),
        );
    }
    Ok(())
}

/// `on` when every table with the tenant policy has row-level security on,
/// `off` when none does, and `partial` otherwise. Whether a role skips it is
/// the role's, not the database's.
fn state(status: &RowSecurity) -> &'static str {
    match (status.on.is_empty(), status.off.is_empty()) {
        (false, true) => "on",
        (true, _) => "off",
        (false, false) => "partial",
    }
}

/// What a Hub says at startup about row-level security on its database:
/// `(warning, text)` pairs. It warns when row-level security is on but this
/// Hub's role skips it, when it's on for only some tables, and when a table
/// with a `tenant_id` has no tenant policy, since then it isn't the backstop
/// it looks like.
pub fn startup_notes(status: &RowSecurity) -> Vec<(bool, String)> {
    let mut notes = Vec::new();
    match state(status) {
        "off" if status.off.is_empty() => {}
        "off" => notes.push((
            false,
            if status.role_bypasses {
                "row-level security is off; this Hub's database role is a superuser or has \
                 BYPASSRLS, so it would skip it anyway"
            } else {
                "row-level security is off: `ctxone-hub db row-security on` switches it on"
            }
            .to_string(),
        )),
        "on" if !status.role_bypasses && status.unguarded.is_empty() => notes.push((
            false,
            format!(
                "row-level security is on for all {} tables that hold tenants' rows",
                status.on.len()
            ),
        )),
        "on" => {}
        _ => notes.push((
            true,
            format!(
                "row-level security is on for only some tables; off for {}. \
                 `ctxone-hub db row-security on` switches it on for the rest",
                status.off.join(", ")
            ),
        )),
    }
    let on = !status.on.is_empty();
    if on && status.role_bypasses {
        notes.push((
            true,
            "row-level security is on, but this Hub's database role is a superuser or has \
             BYPASSRLS, so it skips it: connect as a role with neither"
                .to_string(),
        ));
    }
    if on && !status.unguarded.is_empty() {
        notes.push((
            true,
            format!(
                "row-level security doesn't cover these tables, which have a tenant_id column \
                 and no tenant policy: {}",
                status.unguarded.join(", ")
            ),
        ));
    }
    notes
}

/// Log where row-level security stands on the Hub's database (see
/// [`startup_notes`]).
pub fn log_row_security(status: Result<RowSecurity, StorageError>) {
    match status {
        Ok(status) => {
            for (warning, note) in startup_notes(&status) {
                if warning {
                    tracing::warn!("{note}");
                } else {
                    tracing::info!("{note}");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "couldn't read whether row-level security is on"),
    }
}

/// Tell the person running the command what happened.
fn say(console: &mut Console, line: String) {
    let _ = writeln!(console.err, "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(on: &[&str], off: &[&str], unguarded: &[&str], role_bypasses: bool) -> RowSecurity {
        let names = |t: &[&str]| t.iter().map(|s| s.to_string()).collect();
        RowSecurity {
            on: names(on),
            off: names(off),
            unguarded: names(unguarded),
            role_bypasses,
        }
    }

    fn warnings(status: &RowSecurity) -> Vec<String> {
        startup_notes(status)
            .into_iter()
            .filter(|(warning, _)| *warning)
            .map(|(_, note)| note)
            .collect()
    }

    #[test]
    fn on_for_every_table_and_binding_the_role_is_no_warning() {
        let on = status(&["a", "b"], &[], &[], false);
        assert_eq!(state(&on), "on");
        assert!(warnings(&on).is_empty());
        let notes = startup_notes(&on);
        assert!(notes[0].1.contains("on for all 2 tables"), "{notes:?}");
    }

    #[test]
    fn off_is_no_warning_even_for_a_role_that_would_skip_it() {
        for bypasses in [false, true] {
            let off = status(&[], &["a", "b"], &["c"], bypasses);
            assert_eq!(state(&off), "off");
            assert!(warnings(&off).is_empty(), "{:?}", startup_notes(&off));
            assert_eq!(startup_notes(&off).len(), 1);
        }
    }

    #[test]
    fn on_for_a_role_that_skips_it_warns() {
        let skipped = status(&["a", "b"], &[], &[], true);
        let warned = warnings(&skipped);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(
            warned[0].contains("superuser or has BYPASSRLS"),
            "{warned:?}"
        );
    }

    #[test]
    fn on_for_only_some_tables_warns_and_names_the_rest() {
        let partial = status(&["a"], &["b", "c"], &[], false);
        assert_eq!(state(&partial), "partial");
        let warned = warnings(&partial);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains("off for b, c"), "{warned:?}");
    }

    #[test]
    fn on_with_a_table_the_policy_misses_warns() {
        let unguarded = status(&["a"], &[], &["x"], false);
        let warned = warnings(&unguarded);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains(": x"), "{warned:?}");
    }

    fn run_with(args: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut console = Console {
            out: &mut out,
            err: &mut err,
            ask: None,
        };
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let code = run(&args, &mut console);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn wrong_commands_are_usage_errors_before_any_connection() {
        // A URL nothing listens on: a usage error must come before connecting.
        let url = ["--database-url", "postgres://nobody@127.0.0.1:1/none"];
        for (args, why) in [
            (vec!["row-security"], "needs on, off or status"),
            (
                vec!["row-security", "maybe"],
                "unknown row-security command maybe",
            ),
            (vec!["row-security", "on", "now"], "unexpected now"),
            (vec!["tables"], "unknown command tables"),
            (
                vec!["row-security", "on", "--force"],
                "unknown option --force",
            ),
        ] {
            let mut all = args.clone();
            all.extend(url);
            let (code, _, err) = run_with(&all);
            assert_eq!(code, USAGE, "{all:?}: {err}");
            assert!(err.contains(why), "{all:?}: {err}");
        }
        let (code, out, _) = run_with(&["--help"]);
        assert_eq!(code, OK);
        assert!(out.starts_with("Usage: ctxone-hub db"), "{out}");
    }
}
