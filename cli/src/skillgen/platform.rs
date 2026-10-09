//! The agent/host registry. Generalizes ASD's `TOOLS` table (which only knew
//! MCP-config locations) with the three facts onboarding needs: which always-on
//! instruction file the host loads, and — for hosts with an Agent-Skill concept
//! — where its `SKILL.md` is installed.
//!
//! NB: always-on filenames and skill dirs are host conventions that are still
//! settling; they are validated against real hosts at install time (plan task
//! t-002). The value here is the single point where a new agent is added.

use std::path::{Path, PathBuf};

/// Whether the always-on file is repo-local or in the user's home config.
/// Not part of rendered *content* — used by the installer to choose a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    Home,
}

/// The always-loaded instruction file a host reads. The renderer injects the
/// always-on block into whichever of these a platform uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlwaysOnFile {
    ClaudeMd,
    AgentsMd,
    GeminiMd,
    CopilotInstructions,
    KiroSteering,
    AntigravityRules,
}

impl AlwaysOnFile {
    /// Conventional on-disk filename for this host's always-on file.
    pub fn filename(self) -> &'static str {
        match self {
            AlwaysOnFile::ClaudeMd => "CLAUDE.md",
            AlwaysOnFile::AgentsMd => "AGENTS.md",
            AlwaysOnFile::GeminiMd => "GEMINI.md",
            AlwaysOnFile::CopilotInstructions => ".github/copilot-instructions.md",
            AlwaysOnFile::KiroSteering => ".kiro/steering/AGENTS.md",
            AlwaysOnFile::AntigravityRules => ".antigravity/rules.md",
        }
    }
}

/// Where a host keeps its Agent Skills. Templates use `{slug}` for the skill
/// name and `~` for the user's home. A host that supports skills has a `home`
/// location and, optionally, a repo-local `project` one.
#[derive(Debug, Clone, Copy)]
pub struct SkillDir {
    /// Home-scoped template, e.g. `~/.claude/skills/{slug}`.
    pub home: &'static str,
    /// Optional project-scoped template, e.g. `.claude/skills/{slug}`.
    pub project: Option<&'static str>,
}

impl SkillDir {
    fn expand(template: &str, slug: &str, home: &Path) -> PathBuf {
        let filled = template.replace("{slug}", slug);
        if let Some(rest) = filled.strip_prefix("~/") {
            home.join(rest)
        } else {
            PathBuf::from(filled)
        }
    }

    /// Resolve the home-scoped skill directory for `slug` (e.g.
    /// `/Users/x/.claude/skills/asd`). Returns `None` if `$HOME` is unset.
    pub fn home_dir(&self, slug: &str) -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(self.home_dir_under(slug, Path::new(&home)))
    }

    /// Resolve the home-scoped skill directory against an explicit home dir —
    /// lets callers (and tests) place skills under a chosen root without
    /// mutating the process `$HOME`.
    pub fn home_dir_under(&self, slug: &str, home: &Path) -> PathBuf {
        Self::expand(self.home, slug, home)
    }

    /// Resolve the project-scoped skill directory under `root`, if this host
    /// has one.
    pub fn project_dir(&self, slug: &str, root: &Path) -> Option<PathBuf> {
        self.project.map(|t| root.join(t.replace("{slug}", slug)))
    }
}

/// One host we can onboard into.
#[derive(Debug, Clone, Copy)]
pub struct Platform {
    /// Stable key, e.g. "claude-code".
    pub key: &'static str,
    /// Human name used in rendered skill bodies, e.g. "Claude Code".
    pub display: &'static str,
    /// Which always-on file this host loads.
    pub always_on: AlwaysOnFile,
    /// Where that file lives.
    pub scope: Scope,
    /// Agent-Skill install location, or `None` if the host has no skill concept
    /// (it still gets the always-on block).
    pub skill: Option<SkillDir>,
}

impl Platform {
    /// Whether this host gets a rendered `SKILL.md`.
    pub fn supports_skill(&self) -> bool {
        self.skill.is_some()
    }
}

use AlwaysOnFile::*;
use Scope::*;

/// Convenience for the registry rows.
const fn skill(home: &'static str, project: Option<&'static str>) -> Option<SkillDir> {
    Some(SkillDir { home, project })
}

/// Every host the engine knows. Adding an agent is one row here.
pub const PLATFORMS: &[Platform] = &[
    Platform {
        key: "claude-code",
        display: "Claude Code",
        always_on: ClaudeMd,
        scope: Project,
        skill: skill("~/.claude/skills/{slug}", Some(".claude/skills/{slug}")),
    },
    Platform {
        key: "codex",
        display: "Codex CLI",
        always_on: AgentsMd,
        scope: Project,
        skill: skill("~/.codex/skills/{slug}", None),
    },
    Platform {
        key: "cursor",
        display: "Cursor",
        always_on: AgentsMd,
        scope: Project,
        skill: None,
    },
    Platform {
        key: "gemini-cli",
        display: "Gemini CLI",
        always_on: GeminiMd,
        scope: Project,
        skill: skill("~/.gemini/skills/{slug}", Some(".gemini/skills/{slug}")),
    },
    Platform {
        key: "copilot",
        display: "GitHub Copilot (VS Code)",
        always_on: CopilotInstructions,
        scope: Project,
        skill: None,
    },
    Platform {
        key: "opencode",
        display: "OpenCode",
        always_on: AgentsMd,
        scope: Project,
        skill: skill(
            "~/.config/opencode/skills/{slug}",
            Some(".opencode/skills/{slug}"),
        ),
    },
    Platform {
        key: "kiro",
        display: "Kiro",
        always_on: KiroSteering,
        scope: Project,
        skill: skill("~/.kiro/skills/{slug}", None),
    },
    Platform {
        key: "antigravity",
        display: "Antigravity",
        always_on: AntigravityRules,
        scope: Project,
        skill: None,
    },
    Platform {
        key: "windsurf",
        display: "Windsurf",
        always_on: AgentsMd,
        scope: Project,
        skill: None,
    },
    Platform {
        key: "zed",
        display: "Zed",
        always_on: AgentsMd,
        scope: Project,
        skill: None,
    },
    Platform {
        key: "cline",
        display: "Cline",
        always_on: AgentsMd,
        scope: Project,
        skill: None,
    },
    Platform {
        key: "aider",
        display: "Aider",
        always_on: AgentsMd,
        scope: Project,
        skill: None,
    },
    Platform {
        key: "devin",
        display: "Devin",
        always_on: AgentsMd,
        scope: Home,
        skill: skill("~/.devin/skills/{slug}", None),
    },
    Platform {
        key: "amp",
        display: "Amp",
        always_on: AgentsMd,
        scope: Project,
        skill: skill("~/.config/amp/skills/{slug}", None),
    },
    Platform {
        key: "agents",
        display: "Generic (AGENTS.md)",
        always_on: AgentsMd,
        scope: Home,
        skill: skill("~/.agents/skills/{slug}", None),
    },
];

/// Look up a platform by key.
pub fn platform(key: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|p| p.key == key)
}
