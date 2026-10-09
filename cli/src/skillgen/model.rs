//! The single source of truth a product supplies: what the agent should know
//! and when to reach for the tool. ASD and CTXone each build one `SkillSpec`;
//! the renderer turns it into per-agent artifacts.
//!
//! `SkillSpec` is (de)serializable so one CLI can emit its spec as JSON and the
//! other can read it to render the canonical combined skill.

use serde::{Deserialize, Serialize};

/// One product's onboarding content. Product-neutral: ASD, CTXone, or any
/// future tool fills this in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSpec {
    /// Human name, e.g. "AgentStateDeveloper".
    pub product: String,
    /// Short machine slug, e.g. "asd" / "ctx". Used for the skill dir name and
    /// the always-on marker id (`<!-- asd:begin -->`).
    pub slug: String,
    /// One-line description of what the tool is for.
    pub tagline: String,
    /// Version stamped nowhere in the rendered *content* (renders stay
    /// timestamp/version-free for clean diffs) — carried for the install-time
    /// `.version` file that t-003 writes.
    pub version: String,
    /// Imperative "when to use it" rules — the training that makes the agent
    /// reach for the tool on its own. Order is preserved (author-controlled,
    /// deterministic).
    pub rules: Vec<Rule>,
    /// Key commands/tools worth naming in the skill body.
    pub commands: Vec<CmdHint>,
    /// Optional sibling product to cross-promote (the one-time nudge lands in
    /// t-004; here it's a single forward-compatible line).
    pub sibling: Option<Sibling>,
    /// Ordered steps for the paste-into-your-agent bootstrap (t-006). Empty
    /// means the product has no bootstrap block.
    pub bootstrap: Vec<BootstrapStep>,
}

/// One step in the paste-into-your-agent bootstrap: a shell command the agent
/// runs and a short note on what it does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapStep {
    pub cmd: String,
    pub purpose: String,
}

/// One imperative usage rule, e.g. "Before a non-trivial edit, run
/// `asd prepare-change`."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub text: String,
}

/// A command worth naming, with a one-line purpose.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdHint {
    /// e.g. "asd prepare-change".
    pub invocation: String,
    /// e.g. "scope a change before editing".
    pub purpose: String,
}

/// A sibling product to suggest when it isn't detected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sibling {
    /// e.g. "CTXone".
    pub product: String,
    /// The sibling's executable name, probed on PATH to decide whether it's
    /// already installed (also the one-time-nudge marker key). e.g. "ctx".
    pub bin: String,
    /// One line: what it adds and how to get it.
    pub pitch: String,
}

impl SkillSpec {
    /// Convenience builder for the common shape.
    pub fn new(
        product: impl Into<String>,
        slug: impl Into<String>,
        tagline: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            product: product.into(),
            slug: slug.into(),
            tagline: tagline.into(),
            version: version.into(),
            rules: Vec::new(),
            commands: Vec::new(),
            sibling: None,
            bootstrap: Vec::new(),
        }
    }

    pub fn rule(mut self, text: impl Into<String>) -> Self {
        self.rules.push(Rule { text: text.into() });
        self
    }

    pub fn command(mut self, invocation: impl Into<String>, purpose: impl Into<String>) -> Self {
        self.commands.push(CmdHint {
            invocation: invocation.into(),
            purpose: purpose.into(),
        });
        self
    }

    pub fn sibling(
        mut self,
        product: impl Into<String>,
        bin: impl Into<String>,
        pitch: impl Into<String>,
    ) -> Self {
        self.sibling = Some(Sibling {
            product: product.into(),
            bin: bin.into(),
            pitch: pitch.into(),
        });
        self
    }

    pub fn bootstrap_step(mut self, cmd: impl Into<String>, purpose: impl Into<String>) -> Self {
        self.bootstrap.push(BootstrapStep {
            cmd: cmd.into(),
            purpose: purpose.into(),
        });
        self
    }

    /// Serialize for cross-CLI spec exchange (`skill --emit-spec`).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse a spec emitted by a sibling CLI.
    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}
