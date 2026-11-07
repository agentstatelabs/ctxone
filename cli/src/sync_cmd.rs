//! `ctx remote`, `ctx push`, `ctx pull` and `ctx sync` (ctx-hub-sync t-006).
//!
//! The local Hub is the only writer to its store, so these ask it to push
//! and pull (`/api/sync/*`); it also syncs on its own in the background.
//! A remote's token never goes to the Hub over HTTP or into the store:
//! `--token-stdin` writes it to `~/.ctxone/sync-tokens/<remote>` (mode
//! 0600), which the Hub reads when it syncs, or `--token-env` names an
//! environment variable the Hub's own environment sets.

use std::io::Read;
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::OutputFormat;

#[derive(Subcommand)]
pub enum RemoteAction {
    /// Add a remote Hub, or change where it is. The store defaults to `me`,
    /// the token's own store on a Hub that serves sync.
    Add {
        /// The remote's name, as `ctx remote set` and `ctx push` use it.
        name: String,
        /// The Hub's base URL, such as https://hub.example.com.
        url: String,
        /// The store to sync with on that Hub (default: me).
        #[arg(long)]
        store: Option<String>,
        /// The environment variable holding the token, in the Hub's own
        /// environment.
        #[arg(long, conflicts_with = "token_stdin")]
        token_env: Option<String>,
        /// Read the token from standard input and keep it in
        /// ~/.ctxone/sync-tokens/<name> (mode 0600), never in the store.
        #[arg(long)]
        token_stdin: bool,
    },
    /// List the remotes, and which workspaces sync with each.
    List,
    /// Sync a workspace with a remote. Its history before now stays on this
    /// machine; only what it holds now goes. Setting the first workspace
    /// sets your personal namespace too. For now every workspace syncs to
    /// your own space.
    Set {
        /// The workspace (namespace), such as `default` or a project's.
        namespace: String,
        /// The remote's name.
        remote: String,
    },
    /// Stop syncing a workspace.
    Unset {
        /// The workspace (namespace).
        namespace: String,
    },
    /// Remove a remote, and its token file. No workspace may sync with it.
    Remove {
        /// The remote's name.
        name: String,
    },
}

#[derive(Subcommand)]
pub enum SyncAction {
    /// Where each synced workspace's branches stand against the remote, as
    /// the last sync left them.
    Status,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// `~/.ctxone/sync-tokens/<name>`.
fn token_path(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".ctxone")
            .join("sync-tokens")
            .join(name),
    )
}

/// A remote name the store will take: what AgentStateGraph allows, so a
/// token file is never written under a name it then refuses.
fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('_')
        || name.contains('/')
        || name.contains("..")
        || name.chars().any(char::is_whitespace)
    {
        return Err(format!("not a remote name: {name:?}").into());
    }
    Ok(())
}

fn write_token(name: &str, token: &str) -> Result<PathBuf> {
    let path = token_path(name).ok_or("HOME isn't set, so there's nowhere to keep the token")?;
    let dir = path.parent().expect("a token file has a directory");
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        std::io::Write::write_all(&mut f, token.as_bytes())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, token)?;
    Ok(path)
}

async fn answer(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let text = resp.text().await?;
    if status.is_success() {
        return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
    }
    // The Hub's sync errors are JSON `{code, error}`; others are text.
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or(text);
    Err(format!("{status}: {}", message.trim()).into())
}

pub async fn remote(
    action: RemoteAction,
    server: &str,
    client: &reqwest::Client,
    format: OutputFormat,
) -> Result<()> {
    match action {
        RemoteAction::Add {
            name,
            url,
            store,
            token_env,
            token_stdin,
        } => {
            check_name(&name)?;
            if token_stdin {
                let mut token = String::new();
                std::io::stdin().read_to_string(&mut token)?;
                let token = token.trim();
                if token.is_empty() {
                    return Err("no token came on standard input".into());
                }
                write_token(&name, token)?;
            }
            let body = json!({ "name": name, "url": url, "store": store, "token_env": token_env });
            let v = answer(
                client
                    .post(format!("{server}/api/sync/remotes"))
                    .json(&body)
                    .send()
                    .await?,
            )
            .await?;
            crate::emit(format, &v, |_| {
                println!("remote {name}: {url}");
                if token_stdin {
                    println!("  token kept in ~/.ctxone/sync-tokens/{name}");
                }
                println!("  next: ctx remote set <workspace> {name}");
            });
        }
        RemoteAction::List => {
            let v = answer(
                client
                    .get(format!("{server}/api/sync/remotes"))
                    .send()
                    .await?,
            )
            .await?;
            crate::emit(format, &v, |v| {
                let remotes = v["remotes"].as_array().cloned().unwrap_or_default();
                if remotes.is_empty() {
                    println!("no remotes; ctx remote add <name> <url> --token-stdin");
                }
                for r in remotes {
                    let syncs: Vec<&str> = r["namespaces"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    println!(
                        "{}  {}  store {}{}",
                        r["name"].as_str().unwrap_or("?"),
                        r["url"].as_str().unwrap_or("?"),
                        r["store"].as_str().unwrap_or("?"),
                        if r["has_token"] == true {
                            ""
                        } else {
                            "  (no token)"
                        }
                    );
                    if !syncs.is_empty() {
                        println!("  syncs: {}", syncs.join(", "));
                    }
                }
            });
        }
        RemoteAction::Set { namespace, remote } => {
            let v = answer(
                client
                    .put(format!(
                        "{server}/api/sync/namespaces/{}",
                        crate::urlencoding(&namespace)
                    ))
                    .json(&json!({ "remote": remote }))
                    .send()
                    .await?,
            )
            .await?;
            crate::emit(format, &v, |v| {
                for ns in v["mapped"].as_array().into_iter().flatten() {
                    println!("{} syncs with {remote}", ns.as_str().unwrap_or("?"));
                }
                println!("  its history before now stays on this machine; next: ctx sync");
            });
        }
        RemoteAction::Unset { namespace } => {
            let v = answer(
                client
                    .delete(format!(
                        "{server}/api/sync/namespaces/{}",
                        crate::urlencoding(&namespace)
                    ))
                    .send()
                    .await?,
            )
            .await?;
            crate::emit(format, &v, |v| {
                if v["unmapped"] == true {
                    println!("{namespace} no longer syncs");
                } else {
                    println!("{namespace} didn't sync");
                }
            });
        }
        RemoteAction::Remove { name } => {
            check_name(&name)?;
            let v = answer(
                client
                    .delete(format!(
                        "{server}/api/sync/remotes/{}",
                        crate::urlencoding(&name)
                    ))
                    .send()
                    .await?,
            )
            .await?;
            if let Some(path) = token_path(&name)
                && path.is_file()
            {
                std::fs::remove_file(path)?;
            }
            crate::emit(format, &v, |v| {
                if v["removed"] == true {
                    println!("removed {name}");
                } else {
                    println!("no remote is named {name}");
                }
            });
        }
    }
    Ok(())
}

/// `FINDING=REASON` from `--allow`.
fn overrides(allow: &[String]) -> Result<Vec<Value>> {
    allow
        .iter()
        .map(|a| {
            let (id, reason) = a
                .split_once('=')
                .filter(|(id, reason)| !id.is_empty() && !reason.trim().is_empty())
                .ok_or_else(|| format!("--allow takes FINDING=REASON, not {a:?}"))?;
            Ok(json!({ "finding": id, "reason": reason.trim() }))
        })
        .collect()
}

/// The `ctx forget` that removes the record a finding is in, from the
/// branch it's on.
fn forget_hint(ns: &str, branch: &str, record: &str) -> String {
    let quote = |s: &str| {
        if s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-:@%+=,".contains(c))
        {
            s.to_string()
        } else {
            format!("'{}'", s.replace('\'', r"'\''"))
        }
    };
    if ns == crate::PERSONAL_NAMESPACE {
        return format!("ctx forget --personal {}", quote(record));
    }
    let mut cmd = format!("ctx --namespace {}", quote(ns));
    if branch != "main" {
        cmd.push_str(&format!(" --branch {}", quote(branch)));
    }
    format!("{cmd} forget {}", quote(record))
}

/// `ctx push`, `ctx pull` and `ctx sync`: every synced workspace, or only
/// `namespace`. Returns whether every workspace synced cleanly.
pub async fn run(
    server: &str,
    client: &reqwest::Client,
    format: OutputFormat,
    mode: &str,
    namespace: Option<&str>,
    allow: &[String],
) -> Result<bool> {
    let body = json!({
        "mode": mode,
        "namespace": namespace,
        "overrides": overrides(allow)?,
    });
    let v = answer(
        client
            .post(format!("{server}/api/sync/run"))
            .json(&body)
            .send()
            .await?,
    )
    .await?;
    let workspaces = v["workspaces"].as_array().cloned().unwrap_or_default();
    let clean = workspaces.iter().all(|w| {
        w.get("error").is_none()
            && w["blocked"].as_array().is_none_or(Vec::is_empty)
            && w["errors"].as_array().is_none_or(Vec::is_empty)
    });
    crate::emit(format, &v, |_| {
        if workspaces.is_empty() {
            println!("nothing syncs yet; ctx remote set <workspace> <remote>");
        }
        for w in &workspaces {
            let ns = w["namespace"].as_str().unwrap_or("?");
            let remote = w["remote"].as_str().unwrap_or("?");
            if let Some(e) = w.get("error") {
                println!(
                    "{ns} ⇄ {remote}: {}",
                    e["error"].as_str().unwrap_or("failed")
                );
                continue;
            }
            let pulled: Vec<String> = w["pulled"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["outcome"] != "up_to_date")
                .map(|p| {
                    format!(
                        "{} ({})",
                        p["branch"].as_str().unwrap_or("?"),
                        p["outcome"].as_str().unwrap_or("?")
                    )
                })
                .collect();
            let pushed: Vec<String> = w["pushed"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| {
                    format!(
                        "{} ({} commits)",
                        p["branch"].as_str().unwrap_or("?"),
                        p["commits"].as_u64().unwrap_or(0)
                    )
                })
                .collect();
            println!(
                "{ns} ⇄ {remote}: pulled {}; pushed {}",
                if pulled.is_empty() {
                    "nothing new".to_string()
                } else {
                    pulled.join(", ")
                },
                if pushed.is_empty() {
                    "nothing new".to_string()
                } else {
                    pushed.join(", ")
                }
            );
            for b in w["blocked"].as_array().into_iter().flatten() {
                let branch = b["branch"].as_str().unwrap_or("main");
                println!("  {branch} wasn't pushed: it holds what looks like a credential");
                for f in b["findings"].as_array().into_iter().flatten() {
                    println!(
                        "    {}  ({}, finding {})",
                        f["path"].as_str().unwrap_or("?"),
                        f["rule"].as_str().unwrap_or("?"),
                        f["id"].as_str().unwrap_or("?")
                    );
                    if let Some(record) = f["forget"].as_str() {
                        println!("      remove it: {}", forget_hint(ns, branch, record));
                    }
                    if f["overridable"] == true {
                        println!(
                            "      or push it anyway: ctx push --allow {}='<why it is safe>'",
                            f["id"].as_str().unwrap_or("?")
                        );
                    }
                }
            }
            for e in w["errors"].as_array().into_iter().flatten() {
                let what = e
                    .get("push")
                    .or_else(|| e.get("pull"))
                    .and_then(|x| x["error"].as_str())
                    .unwrap_or("failed");
                println!("  {}: {what}", e["branch"].as_str().unwrap_or("?"));
            }
        }
    });
    Ok(clean)
}

pub async fn status(server: &str, client: &reqwest::Client, format: OutputFormat) -> Result<()> {
    let v = answer(
        client
            .get(format!("{server}/api/sync/status"))
            .send()
            .await?,
    )
    .await?;
    crate::emit(format, &v, |v| {
        let ws = v["workspaces"].as_array().cloned().unwrap_or_default();
        if ws.is_empty() {
            println!("nothing syncs yet; ctx remote set <workspace> <remote>");
        }
        for w in ws {
            let last = match w["last"]["at"].as_str() {
                Some(at) if w["last"]["ok"] == true => format!("last synced {at}"),
                Some(at) => format!("last sync {at} had problems; ctx sync"),
                None => "not synced yet".to_string(),
            };
            println!(
                "{} ⇄ {} ({last})",
                w["namespace"].as_str().unwrap_or("?"),
                w["remote"].as_str().unwrap_or("?")
            );
            for b in w["branches"].as_array().into_iter().flatten() {
                println!(
                    "  {:<24} {}",
                    b["branch"].as_str().unwrap_or("?"),
                    b["state"].as_str().unwrap_or("?").replace('_', " ")
                );
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_need_a_reason() {
        assert_eq!(
            overrides(&["abc123=a fixture".into()]).unwrap(),
            vec![json!({ "finding": "abc123", "reason": "a fixture" })]
        );
        assert!(overrides(&["abc123".into()]).is_err());
        assert!(overrides(&["abc123=  ".into()]).is_err());
    }

    #[test]
    fn a_forget_hint_names_the_workspace_and_branch() {
        assert_eq!(
            forget_hint("repo-a", "main", "/memory/ci/f1"),
            "ctx --namespace repo-a forget /memory/ci/f1"
        );
        assert_eq!(
            forget_hint("repo-a", "feat x", "/memory/ci/f1"),
            "ctx --namespace repo-a --branch 'feat x' forget /memory/ci/f1"
        );
        assert_eq!(
            forget_hint(crate::PERSONAL_NAMESPACE, "main", "/memory/me/f2"),
            "ctx forget --personal /memory/me/f2"
        );
    }

    #[test]
    fn remote_names_are_checked_before_a_token_is_kept() {
        for bad in ["", "_hub", "a/b", "..", "a b"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_name("hub").is_ok());
    }
}
