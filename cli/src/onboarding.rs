//! CTXone onboarding content (suite-onboarding-ctx t-001 / t-005).
//!
//! CTX's single [`SkillSpec`], rendered by the shared `agent-skillgen` engine
//! into CTX's `SKILL.md` + always-on block — mirroring ASD's `asd_skill_spec`.
//! The sibling points back at ASD for the cross-breed nudge, and the same spec
//! feeds the combined ASD+CTX skill (engine `render_combined`).
//!
//! The `ctx skill` install command + the shared install helper are the next
//! step; this module establishes CTX's spec and proves CTX consumes the engine.
#![allow(dead_code)]

use agent_skillgen::SkillSpec;

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
