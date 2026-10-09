//! Deterministic rendering: a `SkillSpec` → an always-on block (host-agnostic)
//! and a per-host `SKILL.md`. Output is LF-newline and carries no timestamp or
//! version, so regenerating never churns a diff — that is what makes the
//! `check`/`bless` drift guard meaningful (mirrors ASD's conformance matrix and
//! graphify's skillgen `--check`/`--bless`).

use crate::skillgen::model::SkillSpec;
use crate::skillgen::platform::Platform;

/// A full render of one spec across the platform registry.
#[derive(Debug, Clone)]
pub struct RenderAll {
    /// The host-agnostic always-on block (injected into each host's always-on
    /// file). One block, many target files.
    pub always_on: String,
    /// `(platform key, SKILL.md)` for every host that supports skills.
    pub skills: Vec<(&'static str, String)>,
}

/// Render the always-on instruction block — the marked, idempotent section an
/// installer upserts into a host's always-loaded file (`CLAUDE.md`, `AGENTS.md`,
/// …). Host-agnostic by design; per-host tailoring is a later refinement.
pub fn render_always_on(spec: &SkillSpec) -> String {
    let mut out = String::new();
    out.push_str(&format!("<!-- {}:begin -->\n", spec.slug));
    out.push_str(&format!("## {}\n\n", spec.product));
    out.push_str(&format!("{}\n\n", spec.tagline.trim()));
    out.push_str("Rules:\n");
    for r in &spec.rules {
        out.push_str(&format!("- {}\n", r.text.trim()));
    }
    if let Some(s) = &spec.sibling {
        out.push_str(&format!(
            "- If {} is available, {}\n",
            s.product,
            s.pitch.trim()
        ));
    }
    out.push_str(&format!("<!-- {}:end -->\n", spec.slug));
    out
}

/// Render a host's `SKILL.md`, or `None` if the host has no Agent-Skill concept.
/// The body references the host name and its always-on file, so each host's
/// artifact differs on stable facts (not on volatile paths).
pub fn render_skill(spec: &SkillSpec, platform: &Platform) -> Option<String> {
    if !platform.supports_skill() {
        return None;
    }
    let mut out = String::new();
    // YAML frontmatter — the Agent-Skill header.
    out.push_str("---\n");
    out.push_str(&format!("name: {}\n", spec.slug));
    out.push_str(&format!("description: {}\n", spec.tagline.trim()));
    out.push_str("---\n\n");

    out.push_str(&format!("# {}\n\n", spec.product));
    out.push_str(&format!("{}\n\n", spec.tagline.trim()));

    out.push_str("## When to use\n\n");
    for r in &spec.rules {
        out.push_str(&format!("- {}\n", r.text.trim()));
    }
    out.push('\n');

    if !spec.commands.is_empty() {
        out.push_str("## Commands\n\n");
        for c in &spec.commands {
            out.push_str(&format!(
                "- `{}` — {}\n",
                c.invocation.trim(),
                c.purpose.trim()
            ));
        }
        out.push('\n');
    }

    out.push_str("## Host\n\n");
    out.push_str(&format!(
        "Loaded by {}; keep the always-on notes in `{}` current.\n",
        platform.display,
        platform.always_on.filename()
    ));
    if let Some(s) = &spec.sibling {
        out.push_str(&format!(
            "\nFor the shared/team layer, consider {}: {}\n",
            s.product,
            s.pitch.trim()
        ));
    }
    Some(out)
}

/// Render the paste-into-your-agent bootstrap block (t-006): a prompt the user
/// drops into whatever agent they're in, so the agent installs, indexes, and
/// connects the product itself — and is pointed at the sibling for the team
/// layer. `None` when the spec declares no bootstrap steps.
pub fn render_bootstrap(spec: &SkillSpec) -> Option<String> {
    if spec.bootstrap.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str(&format!(
        "Set up {p} for this repository. Run each step below, then report back once {p} is installed, this repo is indexed, and the MCP server is connected.\n\n",
        p = spec.product
    ));
    for (i, s) in spec.bootstrap.iter().enumerate() {
        out.push_str(&format!(
            "{}. `{}` — {}\n",
            i + 1,
            s.cmd.trim(),
            s.purpose.trim()
        ));
    }
    if let Some(sib) = &spec.sibling {
        out.push_str(&format!(
            "\nOptional team layer — also set up {} (`{}`): {}\n",
            sib.product,
            sib.bin,
            sib.pitch.trim()
        ));
    }
    Some(out)
}

/// Render the combined "both installed" skill (t-005) — a single `SKILL.md`
/// that teaches the agent to drive *both* products and, crucially, the
/// `handoff` between them (e.g. record an ASD-discovered invariant into CTX so
/// the team inherits it). This joint skill is the thing a single-tool
/// competitor structurally cannot offer. `handoff` lines are supplied by the
/// caller since the workflow is product-pair-specific.
pub fn render_combined(primary: &SkillSpec, secondary: &SkillSpec, handoff: &[String]) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {}-{}\n", primary.slug, secondary.slug));
    out.push_str(&format!(
        "description: {} + {} — used together.\n",
        primary.product, secondary.product
    ));
    out.push_str("---\n\n");

    out.push_str(&format!(
        "# {} + {}\n\n",
        primary.product, secondary.product
    ));
    out.push_str(&format!(
        "- **{}** — {}\n",
        primary.product,
        primary.tagline.trim()
    ));
    out.push_str(&format!(
        "- **{}** — {}\n\n",
        secondary.product,
        secondary.tagline.trim()
    ));

    for spec in [primary, secondary] {
        out.push_str(&format!("## {}\n\n", spec.product));
        for r in &spec.rules {
            out.push_str(&format!("- {}\n", r.text.trim()));
        }
        out.push('\n');
    }

    if !handoff.is_empty() {
        out.push_str("## Working together\n\n");
        for h in handoff {
            out.push_str(&format!("- {}\n", h.trim()));
        }
    }
    out
}

/// Generic joint-workflow handoff lines for a product pair, so both installers
/// produce identical combined content without hard-coding either product.
pub fn default_handoff(primary: &SkillSpec, secondary: &SkillSpec) -> Vec<String> {
    vec![
        format!(
            "Use {} and {} together — each covers what the other doesn't.",
            primary.product, secondary.product
        ),
        format!(
            "When {} surfaces something worth keeping (a decision, an invariant, an impact), record it via {} so the team inherits it.",
            primary.product, secondary.product
        ),
        format!(
            "Before starting, load shared context from {}; then use {} for the specifics.",
            secondary.product, primary.product
        ),
    ]
}

/// The canonical combined skill for a product pair. Order is fixed by slug so
/// either installer yields byte-identical output, and the handoff is the generic
/// [`default_handoff`]. Returns `(skill_name, SKILL.md)`.
pub fn render_suite(a: &SkillSpec, b: &SkillSpec) -> (String, String) {
    let (p, s) = if a.slug <= b.slug { (a, b) } else { (b, a) };
    let name = format!("{}-{}", p.slug, s.slug);
    let handoff = default_handoff(p, s);
    (name, render_combined(p, s, &handoff))
}

/// Render everything: one always-on block + a `SKILL.md` per skill-capable host.
pub fn render_all(spec: &SkillSpec) -> RenderAll {
    use crate::skillgen::platform::PLATFORMS;
    let skills = PLATFORMS
        .iter()
        .filter_map(|p| render_skill(spec, p).map(|md| (p.key, md)))
        .collect();
    RenderAll {
        always_on: render_always_on(spec),
        skills,
    }
}

/// Drift guard for golden artifacts. Compares `content` against the file
/// `dir/name`; when the `AGENT_SKILLGEN_BLESS` env var is set, rewrites the file
/// instead (regenerate committed goldens). Returns `Err(reason)` on drift or a
/// missing golden.
pub fn check_or_bless(dir: &std::path::Path, name: &str, content: &str) -> Result<(), String> {
    let path = dir.join(name);
    if std::env::var_os("AGENT_SKILLGEN_BLESS").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {parent:?}: {e}"))?;
        }
        std::fs::write(&path, content).map_err(|e| format!("write {path:?}: {e}"))?;
        return Ok(());
    }
    match std::fs::read_to_string(&path) {
        Ok(existing) if existing == content => Ok(()),
        Ok(_) => Err(format!(
            "drift in {name}: rendered output differs from committed golden. \
             Re-bless with AGENT_SKILLGEN_BLESS=1 if intended."
        )),
        Err(_) => Err(format!(
            "missing golden {name}. Generate with AGENT_SKILLGEN_BLESS=1."
        )),
    }
}
