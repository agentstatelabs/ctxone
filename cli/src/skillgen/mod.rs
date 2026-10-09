//! Vendored copy of `agent-skillgen` v0.1.0
//! (<https://github.com/agentstatelabs/agent-skillgen>, commit be42d5d).
//!
//! crates.io rejects git dependencies, and agent-skillgen is not published
//! there, so the engine is copied in rather than depended on.
//! AgentStateDeveloper carries the same copy. Change it upstream first, then
//! re-copy it here and in AgentStateDeveloper, so the two CLIs keep rendering
//! identical skill files. The only local edits
//! are the `crate::skillgen::` paths, rustfmt's edition-2024 import order,
//! and the `#[allow]` on the `mod skillgen` line in main.rs.
//!
//! Upstream crate docs follow.
//!
//! # agent-skillgen
//!
//! Shared agent-onboarding engine for the AgentStateLabs suite. Both
//! AgentStateDeveloper (ASD) and CTXone consume it: each supplies one
//! [`SkillSpec`] describing what the agent should know and when to reach for the
//! tool, and the engine renders per-agent `SKILL.md` files plus a host-agnostic
//! always-on instruction block — from **one source**, across the whole
//! [`platform::PLATFORMS`] registry.
//!
//! Design (mirrors graphify's `skillgen` and ASD's conformance-matrix-as-spec):
//! - **One source** — a `SkillSpec` per product (not per agent).
//! - **Deterministic render** — LF newlines, no timestamp/version in content,
//!   author-ordered rules; regenerating never churns a diff.
//! - **Drift guard** — [`render::check_or_bless`] compares against committed
//!   golden artifacts and fails on drift; `AGENT_SKILLGEN_BLESS=1` regenerates.
//!
//! What lives elsewhere: the *actual* ASD/CTX content (their `SkillSpec`s), the
//! install-time file placement, version stamping, and the cross-product nudge
//! are downstream plan tasks (suite-onboarding t-002/t-003/t-004/t-005). This
//! crate is the reusable substrate they all build on.

pub mod detect;
pub mod install;
pub mod model;
pub mod platform;
pub mod render;
pub mod state;

pub use detect::{already_nudged, binary_on_path, record_nudge, should_nudge};
pub use install::{
    Action, Placed, SkillScope, install_suite, place_rendered, place_skills, skill_status,
};
pub use model::{BootstrapStep, CmdHint, Rule, Sibling, SkillSpec};
pub use platform::{AlwaysOnFile, PLATFORMS, Platform, Scope, SkillDir, platform};
pub use render::{
    RenderAll, check_or_bless, default_handoff, render_all, render_always_on, render_bootstrap,
    render_combined, render_skill, render_suite,
};
pub use state::{STAMP_FILE, SkillState, compare_versions, read_stamp, skill_state, write_stamp};

/// A fixed, product-neutral spec used by the golden tests and as living
/// documentation of the model. Deliberately NOT ASD/CTX content — those specs
/// live in their own repos (plan tasks t-002/t-005/t-007). Changing this
/// example is expected to re-bless the goldens.
pub fn example_spec() -> SkillSpec {
    SkillSpec::new(
        "DemoTool",
        "demo",
        "A code-context tool for coding agents.",
        "1.0.0",
    )
    .rule("Before a non-trivial edit, run `demo prepare` to scope it.")
    .rule("Before assuming a symbol's behavior, run `demo explain <symbol>`.")
    .rule("After changing code, run `demo reindex` to keep context current.")
    .command("demo prepare", "scope a change before editing")
    .command("demo explain", "get focused context for a symbol")
    .bootstrap_step("brew install demo", "install the CLI")
    .bootstrap_step("demo index .", "index this repo")
    .sibling(
        "SiblingTool",
        "sibling",
        "the shared/team layer — install it to share decisions across the team.",
    )
}
