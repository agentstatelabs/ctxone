//! `ctx worktree` — plan-scoped git worktrees (agent-worktree-workflow t-001).
//!
//! Each unit of work gets its own worktree + `plan/<name>` branch, so parallel
//! agents get isolated files + HEAD (they can't clobber each other) while still
//! sharing the CTXone brain (workspace is per-repo, not per-checkout). Lifecycle:
//!   ctx worktree start <plan>   -> add ../<repo>-wt-<plan> on plan/<plan>
//!   (work there)                -> the CLI can't cd you; open your session in it
//!   ctx worktree finish <plan>  -> merge back to main + MANDATORY teardown
//!
//! The plan<->worktree binding is by CONVENTION (dir `<repo>-wt-<plan>`, branch
//! `plan/<plan>`), so no server/graph write is needed; `list` recovers it from
//! `git worktree list`.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Subcommand;

type Res = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, Subcommand)]
pub enum WorktreeAction {
    /// Create a plan-scoped worktree + `plan/<name>` branch, then print its path.
    Start {
        /// Plan name (kebab-case).
        plan: String,
        /// Ref to branch from.
        #[arg(long, default_value = "main")]
        from: String,
        /// Share one Rust build cache across worktrees: write a
        /// `.cargo/config.toml` pointing `target-dir` at `<repo>/.wt-target`,
        /// avoiding a multi-GB `target/` per worktree. Off by default
        /// (per-worktree target, removed on `finish`).
        #[arg(long)]
        shared_target: bool,
        /// Isolate via a fresh CLONE (its own .git) into `../<repo>-clone-<plan>`
        /// instead of a worktree — for remote/cloud agents on another machine.
        /// Merge-back is via origin (push the branch), not a local merge.
        #[arg(long)]
        clone: bool,
        /// Clone URL (default: this repo's `origin` remote). Only with --clone.
        #[arg(long)]
        url: Option<String>,
    },
    /// List this repo's plan-scoped worktrees and clones.
    List,
    /// Merge the plan's branch back into the target branch, then tear the
    /// worktree down (force-remove + delete branch + prune). Run from anywhere;
    /// operates on the main checkout.
    Finish {
        /// Plan name.
        plan: String,
        /// `git push` the target branch after merging.
        #[arg(long)]
        push: bool,
        /// Merge but KEEP the worktree + branch (skip teardown).
        #[arg(long)]
        keep: bool,
        /// Branch to merge into.
        #[arg(long, default_value = "main")]
        into: String,
        /// Finish a --clone workspace: push the branch to origin (with --push),
        /// then remove the clone directory. Merge to main happens on origin/MR.
        #[arg(long)]
        clone: bool,
    },
}

/// Derived worktree layout for a plan. Pure — unit-testable.
#[derive(Debug, PartialEq)]
struct Layout {
    repo_name: String,
    wt_dir: PathBuf,
    branch: String,
}

/// `<parent>/<repo>-wt-<plan>` on branch `plan/<plan>`.
fn layout(root: &Path, plan: &str) -> Layout {
    let repo_name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let parent = root.parent().unwrap_or(root);
    let wt_dir = parent.join(format!("{repo_name}-wt-{plan}"));
    Layout {
        repo_name,
        wt_dir,
        branch: format!("plan/{plan}"),
    }
}

fn git(args: &[&str], cwd: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(Command::new("git").args(args).current_dir(cwd).output()?)
}

fn git_checked(args: &[&str], cwd: &Path) -> Res {
    let out = git(args, cwd)?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn git_stdout(args: &[&str], cwd: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let out = git(args, cwd)?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The MAIN worktree's path (the first entry of `git worktree list`), resolved
/// from the current directory. Linked worktrees and the main checkout all
/// report the same list, so `finish` can operate on main regardless of cwd —
/// and crucially, without ever switching a shared checkout's HEAD.
fn main_root(cwd: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let list = git_stdout(&["worktree", "list", "--porcelain"], cwd)?;
    for line in list.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            return Ok(PathBuf::from(p));
        }
    }
    Err("not inside a git repository".into())
}

pub fn run(action: WorktreeAction) -> Res {
    let cwd = std::env::current_dir()?;
    let root = main_root(&cwd)?;
    match action {
        WorktreeAction::Start {
            plan,
            from,
            shared_target,
            clone,
            url,
        } => {
            if clone {
                start_clone(&root, &plan, url.as_deref())
            } else {
                start(&root, &plan, &from, shared_target)
            }
        }
        WorktreeAction::List => list(&root),
        WorktreeAction::Finish {
            plan,
            push,
            keep,
            into,
            clone,
        } => {
            if clone {
                finish_clone(&root, &plan, push)
            } else {
                finish(&root, &plan, &into, push, keep)
            }
        }
    }
}

/// `<parent>/<repo>-clone-<plan>` — the isolated clone directory.
fn clone_dir(root: &Path, plan: &str) -> PathBuf {
    let repo_name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    root.parent()
        .unwrap_or(root)
        .join(format!("{repo_name}-clone-{plan}"))
}

fn start_clone(root: &Path, plan: &str, url: Option<&str>) -> Res {
    let dir = clone_dir(root, plan);
    let branch = format!("plan/{plan}");
    if dir.exists() {
        println!("Clone already exists: {}", dir.display());
        println!("  cd {}", dir.display());
        return Ok(());
    }
    let url = match url {
        Some(u) => u.to_string(),
        None => git_stdout(&["remote", "get-url", "origin"], root)?,
    };
    if url.is_empty() {
        return Err("no `origin` remote — pass --url <remote>".into());
    }
    let parent = dir.parent().unwrap_or(root);
    git_checked(&["clone", &url, &dir.to_string_lossy()], parent)?;
    git_checked(&["checkout", "-b", &branch], &dir)?;
    println!("\u{2713} clone {} on branch {}", dir.display(), branch);
    println!("  Open your agent/session there:  cd {}", dir.display());
    println!("  When done:  ctx worktree finish {plan} --clone --push");
    Ok(())
}

fn finish_clone(root: &Path, plan: &str, push: bool) -> Res {
    let dir = clone_dir(root, plan);
    let branch = format!("plan/{plan}");
    if !dir.exists() {
        return Err(format!(
            "no clone at {} — run `ctx worktree start {plan} --clone` first",
            dir.display()
        )
        .into());
    }
    let dirty = git_stdout(&["status", "--porcelain"], &dir)?;
    if !dirty.is_empty() {
        return Err(format!(
            "clone {} has uncommitted changes — commit or stash them first",
            dir.display()
        )
        .into());
    }
    if push {
        git_checked(&["push", "origin", &branch], &dir)?;
        println!("\u{2713} pushed {branch} to origin (merge it via MR / on the main checkout)");
    } else {
        println!("  (not pushed — pass --push to publish {branch} before teardown)");
    }
    // A clone is a plain directory (its own .git), so teardown is a recursive
    // remove — not `git worktree remove`.
    std::fs::remove_dir_all(&dir)?;
    println!("\u{2713} removed clone {}", dir.display());
    Ok(())
}

fn start(root: &Path, plan: &str, from: &str, shared_target: bool) -> Res {
    let l = layout(root, plan);
    if l.wt_dir.exists() {
        println!("Worktree already exists: {}", l.wt_dir.display());
        println!("  cd {}", l.wt_dir.display());
        return Ok(());
    }
    let wt = l.wt_dir.to_string_lossy().into_owned();
    git_checked(&["worktree", "add", &wt, "-b", &l.branch, from], root)?;

    if shared_target {
        let cargo_dir = l.wt_dir.join(".cargo");
        std::fs::create_dir_all(&cargo_dir)?;
        let target = root.join(".wt-target");
        std::fs::write(
            cargo_dir.join("config.toml"),
            format!("[build]\ntarget-dir = \"{}\"\n", target.display()),
        )?;
    }

    println!(
        "\u{2713} worktree {} on branch {}",
        l.wt_dir.display(),
        l.branch
    );
    println!(
        "  Open your agent/session there:  cd {}",
        l.wt_dir.display()
    );
    if shared_target {
        println!("  Shared build cache: {}/.wt-target", root.display());
    }
    println!("  When done:  ctx worktree finish {plan}");
    Ok(())
}

fn list(root: &Path) -> Res {
    let text = git_stdout(&["worktree", "list", "--porcelain"], root)?;
    let prefix = format!(
        "{}-wt-",
        root.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    );

    // Parse porcelain blocks: `worktree <path>` then `branch refs/heads/<b>`.
    let mut path: Option<String> = None;
    let mut branch: Option<String> = None;
    let mut rows: Vec<(String, String, String)> = Vec::new();
    let mut flush = |path: &Option<String>, branch: &Option<String>| {
        if let Some(p) = path
            && let Some(name) = Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
            && let Some(plan) = name.strip_prefix(&prefix)
        {
            rows.push((
                plan.to_string(),
                branch.clone().unwrap_or_else(|| "?".into()),
                p.clone(),
            ));
        }
    };
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            flush(&path, &branch);
            path = Some(p.to_string());
            branch = None;
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.trim_start_matches("refs/heads/").to_string());
        }
    }
    flush(&path, &branch);

    // Clones have their own .git and aren't in `git worktree list`; scan siblings.
    let repo_name = prefix.trim_end_matches("-wt-").to_string();
    let clone_prefix = format!("{repo_name}-clone-");
    let mut clone_rows: Vec<(String, String)> = Vec::new();
    if let Some(parent) = root.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(plan) = name.strip_prefix(&clone_prefix)
                && e.path().is_dir()
            {
                clone_rows.push((plan.to_string(), e.path().to_string_lossy().into_owned()));
            }
        }
    }
    clone_rows.sort();

    if rows.is_empty() && clone_rows.is_empty() {
        println!("No plan-scoped worktrees or clones for {repo_name}.");
        return Ok(());
    }
    for (plan, branch, p) in rows {
        println!("  wt     {plan:<20} [{branch}]  {p}");
    }
    for (plan, p) in clone_rows {
        println!("  clone  {plan:<20} [plan/{plan}]  {p}");
    }
    Ok(())
}

fn finish(root: &Path, plan: &str, into: &str, push: bool, keep: bool) -> Res {
    let l = layout(root, plan);
    if !l.wt_dir.exists() {
        return Err(format!(
            "no worktree at {} — run `ctx worktree start {plan}` first",
            l.wt_dir.display()
        )
        .into());
    }

    // Refuse if the worktree has uncommitted work — nothing should be lost.
    let dirty = git_stdout(&["status", "--porcelain"], &l.wt_dir)?;
    if !dirty.is_empty() {
        return Err(format!(
            "worktree {} has uncommitted changes — commit or stash them first",
            l.wt_dir.display()
        )
        .into());
    }

    // Merge into the target branch on the MAIN checkout — but never switch its
    // HEAD (that's the shared-checkout hazard this whole tool exists to avoid).
    // Require main to already be on `into`.
    let cur = git_stdout(&["rev-parse", "--abbrev-ref", "HEAD"], root)?;
    if cur != into {
        return Err(format!(
            "main checkout {} is on '{cur}', not '{into}' — checkout {into} there first \
             (refusing to switch it for you: another agent may be using it)",
            root.display()
        )
        .into());
    }

    let msg = format!("Merge {} ({plan})", l.branch);
    let merge = git(&["merge", "--no-ff", &l.branch, "-m", &msg], root)?;
    if !merge.status.success() {
        let _ = git(&["merge", "--abort"], root);
        return Err(format!(
            "merge conflict merging {} into {into} — resolve manually:\n{}",
            l.branch,
            String::from_utf8_lossy(&merge.stderr).trim()
        )
        .into());
    }
    println!("\u{2713} merged {} into {into}", l.branch);

    if push {
        git_checked(&["push", "origin", into], root)?;
        println!("\u{2713} pushed {into}");
    }

    if keep {
        println!("  (kept worktree + branch; run finish again without --keep to tear down)");
        return Ok(());
    }

    // Mandatory teardown — force because worktree remove refuses on untracked/
    // ignored files (e.g. a Rust target/), the recurring manual-cleanup pain.
    git_checked(
        &["worktree", "remove", "--force", &l.wt_dir.to_string_lossy()],
        root,
    )?;
    git_checked(&["branch", "-D", &l.branch], root)?;
    let _ = git(&["worktree", "prune"], root);
    println!(
        "\u{2713} removed worktree {} and branch {}",
        l.wt_dir.display(),
        l.branch
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_derives_sibling_dir_and_plan_branch() {
        let l = layout(Path::new("/home/me/CTXone"), "cost-per-feature");
        assert_eq!(l.repo_name, "CTXone");
        assert_eq!(
            l.wt_dir,
            PathBuf::from("/home/me/CTXone-wt-cost-per-feature")
        );
        assert_eq!(l.branch, "plan/cost-per-feature");
    }

    #[test]
    fn layout_handles_root_without_parent() {
        // Degenerate path still produces a stable branch name.
        let l = layout(Path::new("/repo"), "x");
        assert_eq!(l.branch, "plan/x");
        assert_eq!(l.wt_dir, PathBuf::from("/repo-wt-x"));
    }
}
