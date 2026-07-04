//! CTXone onboarding content (suite-onboarding-ctx t-001 / t-005).
//!
//! CTX's single [`SkillSpec`], rendered by the shared `agent-skillgen` engine
//! into CTX's `SKILL.md` + always-on block — mirroring ASD's `asd_skill_spec`.
//! The sibling points back at ASD for the cross-breed nudge, and the same spec
//! feeds the combined ASD+CTX skill (engine `render_combined`).
//!
//! `ctx skill` installs CTX's SKILL.md into each host via the shared engine,
//! with the reverse CTX→ASD nudge. `suite_handoff()` feeds the combined skill.
#![allow(dead_code)] // suite_handoff is used by the (pending) combined-skill install

use std::path::{Path, PathBuf};

use agent_skillgen::{
    Action, SkillScope, SkillSpec, SkillState, already_nudged, binary_on_path, place_skills,
    record_nudge, should_nudge, skill_status,
};

/// CTXone's onboarding content — the single source the shared engine renders
/// into per-agent skill files and the always-on block.
pub fn ctx_skill_spec() -> SkillSpec {
    SkillSpec::new(
        "CTXone",
        "ctx",
        "Shared team memory, plans, and decisions for coding agents.",
        env!("CARGO_PKG_VERSION"),
    )
    .rule("Before starting work, load shared context — check relevant plans and pinned memory.")
    .rule("When you make or learn a decision, record it with `ctx remember \"<decision>\"` so the team inherits it.")
    .rule("Track multi-step work as a plan (`ctx plan add`), and mark tasks done as you complete them.")
    .rule("At the end of a work session, run `ctx summarize-session` to capture outcomes for the team.")
    .command("ctx remember", "record a decision or fact into shared memory")
    .command("ctx plan", "create and track a multi-step plan")
    .command("ctx summarize-session", "capture a session's outcomes for the team")
    .sibling(
        "AgentStateDeveloper",
        "asd",
        "use it for code structure, impact analysis, and change scoping (the `asd` CLI).",
    )
    .bootstrap_step("brew install ctxone", "install the ctx CLI")
    .bootstrap_step("ctx agents install --yes", "prime the shared AGENTS.md in the Hub")
}

/// The handoff lines for the combined ASD+CTX skill, from CTX's perspective —
/// how the two products work together (engine `render_combined`).
pub fn suite_handoff() -> Vec<String> {
    vec![
        "Use ASD for the code specifics (structure, impact, invariants); use CTXone to share what you decide.".to_string(),
        "After ASD surfaces an invariant or a change's blast radius, record the decision in CTXone with `ctx remember` so the team inherits it.".to_string(),
        "Before starting, load the team's shared plans/decisions from CTXone; then use ASD to navigate the code.".to_string(),
    ]
}

/// Install CTXone's Agent Skill (`SKILL.md`) into each skill-capable host via
/// the shared engine — CTX's mirror of `asd skill` (suite-onboarding-ctx t-002),
/// with the reverse CTX→ASD one-time nudge (t-004) and a post-install verify.
pub fn run_skill(
    project: bool,
    tool: Option<&str>,
    remove: bool,
    status: bool,
    no_nudge: bool,
    dry_run: bool,
) -> std::io::Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "cannot resolve $HOME"))?;
    let root = std::env::current_dir()?;
    let scope = if project {
        SkillScope::Project
    } else {
        SkillScope::Home
    };
    let spec = ctx_skill_spec();

    if status {
        let states = skill_status(&spec, &home, &root, scope, tool);
        if states.is_empty() {
            println!("No skill-capable hosts matched.");
        }
        for (t, s) in &states {
            println!("  {t:<12}  {}", describe_state(s));
        }
        return Ok(());
    }

    let placed = place_skills(&spec, &home, &root, scope, tool, remove, dry_run)?;
    if placed.is_empty() {
        println!("No skill-capable hosts matched.");
        return Ok(());
    }
    for p in &placed {
        let verb = match p.action {
            Action::Wrote => "installed",
            Action::Removed => "removed",
            Action::WouldWrite => "would install",
            Action::WouldRemove => "would remove",
            Action::Skipped => "skipped",
            Action::SkippedNewer => "skipped (newer on disk)",
        };
        println!("  {verb:>22}  {:<12}  {}", p.tool, p.path.display());
    }

    if !dry_run && !remove {
        // Self-verify.
        let states = skill_status(&spec, &home, &root, scope, tool);
        let total = states.len();
        let current = states
            .iter()
            .filter(|(_, s)| matches!(s, SkillState::Current { .. }))
            .count();
        if current == total {
            println!("\n✓ verified {current}/{total} skills installed and current");
        } else {
            println!("\n⚠ verified {current}/{total} current — issues:");
            for (t, s) in &states {
                if !matches!(s, SkillState::Current { .. }) {
                    println!("    {t:<12}  {}", describe_state(s));
                }
            }
        }
        // One-time CTX→ASD nudge (reverse of ASD's ASD→CTX).
        let suppress = no_nudge || std::env::var_os("CTX_NO_SUGGEST").is_some();
        if let Some(msg) = maybe_nudge_sibling(&spec, &ctx_state_dir(&home), suppress) {
            println!("{msg}");
        }
    }
    Ok(())
}

/// Print CTX's paste-into-your-agent bootstrap block (suite-onboarding-ctx
/// t-006) — the agent installs + primes CTX, and is pointed at ASD.
pub fn run_bootstrap() {
    match agent_skillgen::render_bootstrap(&ctx_skill_spec()) {
        Some(block) => print!("{block}"),
        None => println!("No bootstrap steps are defined."),
    }
}

fn ctx_state_dir(home: &Path) -> PathBuf {
    home.join(".config").join("ctxone")
}

fn describe_state(state: &SkillState) -> String {
    match state {
        SkillState::NotInstalled => "not installed".to_string(),
        SkillState::Missing => "SKILL.md missing — run `ctx skill` to repair".to_string(),
        SkillState::Unstamped => "installed, no version stamp".to_string(),
        SkillState::Current { version } => format!("current ({version})"),
        SkillState::Stale { installed, package } => {
            format!("stale ({installed} < {package}) — run `ctx skill` to update")
        }
        SkillState::Newer { installed, package } => {
            format!("newer on disk ({installed} > {package}) — not overwriting")
        }
    }
}

fn maybe_nudge_sibling(spec: &SkillSpec, state_dir: &Path, suppressed: bool) -> Option<String> {
    let sib = spec.sibling.as_ref()?;
    let present = binary_on_path(&sib.bin);
    if !should_nudge(present, already_nudged(state_dir, &sib.bin), suppressed) {
        return None;
    }
    let _ = record_nudge(state_dir, &sib.bin);
    Some(format!(
        "\nTip: pair CTXone with {} — {}\n(Shown once; suppress with --no-nudge or CTX_NO_SUGGEST=1.)",
        sib.product, sib.pitch
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_skillgen::{platform, render_bootstrap, render_combined, render_skill};

    #[test]
    fn ctx_spec_renders_real_content() {
        let spec = ctx_skill_spec();
        let md = render_skill(&spec, platform("claude-code").unwrap()).unwrap();
        assert!(md.contains("CTXone"));
        assert!(md.contains("ctx remember"));
        assert!(
            md.contains("AgentStateDeveloper"),
            "sibling cross-promo present"
        );
        assert!(md.starts_with("---\nname: ctx\n"));
    }

    #[test]
    fn ctx_bootstrap_offers_the_suite() {
        let block = render_bootstrap(&ctx_skill_spec()).unwrap();
        assert!(block.contains("ctx agents install"));
        assert!(
            block.contains("AgentStateDeveloper"),
            "dual bootstrap points at ASD"
        );
    }

    #[test]
    fn combined_skill_teaches_both() {
        // CTX can build the joint skill from its own spec + a minimal ASD spec.
        let asd = SkillSpec::new(
            "AgentStateDeveloper",
            "asd",
            "Code-level context and impact analysis.",
            "1.0.0",
        )
        .rule("Run `asd prepare-change` to scope a change.");
        let combined = render_combined(&asd, &ctx_skill_spec(), &suite_handoff());
        assert!(combined.contains("# AgentStateDeveloper + CTXone"));
        assert!(combined.contains("## Working together"));
        assert!(combined.contains("ctx remember"));
    }
}
