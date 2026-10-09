//! Skill-file installation, shared by every product's installer. Renders a
//! spec and places a `SKILL.md` into each skill-capable host's directory, with
//! version stamping and a refusal to overwrite a newer on-disk skill. Paths
//! resolve against an explicit `home`/`root` (not the process env) so callers
//! and tests target any directory.
//!
//! Dependency-light: returns [`std::io::Result`] rather than pulling an error
//! crate. Callers wrap with their own context.

use std::path::{Path, PathBuf};

use crate::skillgen::model::SkillSpec;
use crate::skillgen::platform::PLATFORMS;
use crate::skillgen::render::{render_skill, render_suite};
use crate::skillgen::state::{SkillState, skill_state, write_stamp};

/// Where to install: user-wide (home) or repo-local (project).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillScope {
    Home,
    Project,
}

/// What happened for one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Wrote,
    Removed,
    WouldWrite,
    WouldRemove,
    Skipped,
    /// The on-disk skill is newer than this package — refused to overwrite.
    SkippedNewer,
}

/// Placement outcome for one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub tool: &'static str,
    pub path: PathBuf,
    pub action: Action,
}

/// Render `spec` and place a `SKILL.md` into each skill-capable host.
pub fn place_skills(
    spec: &SkillSpec,
    home: &Path,
    root: &Path,
    scope: SkillScope,
    tool_filter: Option<&str>,
    remove: bool,
    dry_run: bool,
) -> std::io::Result<Vec<Placed>> {
    let mut out = Vec::new();
    for p in PLATFORMS {
        if let Some(f) = tool_filter {
            if p.key != f {
                continue;
            }
        }
        let Some(skill) = p.skill else { continue };
        let dir = match scope {
            SkillScope::Home => skill.home_dir_under(&spec.slug, home),
            SkillScope::Project => match skill.project_dir(&spec.slug, root) {
                Some(d) => d,
                None => continue, // host has no project-scoped skill location
            },
        };
        let path = dir.join("SKILL.md");

        if remove {
            let action = if dry_run {
                Action::WouldRemove
            } else if path.exists() {
                std::fs::remove_file(&path)?;
                let _ = std::fs::remove_dir(&dir); // best-effort if now empty
                Action::Removed
            } else {
                Action::Skipped
            };
            out.push(Placed {
                tool: p.key,
                path,
                action,
            });
            continue;
        }

        let Some(content) = render_skill(spec, p) else {
            continue;
        };
        // Version safety: never overwrite a strictly-newer installed skill.
        if skill_state(&dir, &spec.version).is_downgrade() {
            out.push(Placed {
                tool: p.key,
                path,
                action: Action::SkippedNewer,
            });
            continue;
        }
        let action = if dry_run {
            Action::WouldWrite
        } else {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, &content)?;
            write_stamp(&dir, &spec.version)?;
            Action::Wrote
        };
        out.push(Placed {
            tool: p.key,
            path,
            action,
        });
    }
    Ok(out)
}

/// Place pre-rendered `content` as `SKILL.md` under `slug` in each skill-capable
/// host — for the combined suite skill, whose content isn't a single-spec
/// render. Mirrors [`place_skills`] (version stamp + downgrade refusal) but
/// writes the supplied content instead of rendering one.
#[allow(clippy::too_many_arguments)]
pub fn place_rendered(
    slug: &str,
    version: &str,
    content: &str,
    home: &Path,
    root: &Path,
    scope: SkillScope,
    tool_filter: Option<&str>,
    dry_run: bool,
) -> std::io::Result<Vec<Placed>> {
    let mut out = Vec::new();
    for p in PLATFORMS {
        if let Some(f) = tool_filter {
            if p.key != f {
                continue;
            }
        }
        let Some(skill) = p.skill else { continue };
        let dir = match scope {
            SkillScope::Home => skill.home_dir_under(slug, home),
            SkillScope::Project => match skill.project_dir(slug, root) {
                Some(d) => d,
                None => continue,
            },
        };
        let path = dir.join("SKILL.md");
        if skill_state(&dir, version).is_downgrade() {
            out.push(Placed {
                tool: p.key,
                path,
                action: Action::SkippedNewer,
            });
            continue;
        }
        let action = if dry_run {
            Action::WouldWrite
        } else {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, content)?;
            write_stamp(&dir, version)?;
            Action::Wrote
        };
        out.push(Placed {
            tool: p.key,
            path,
            action,
        });
    }
    Ok(out)
}

/// Install the canonical combined suite skill for `own` + `sibling`. Because
/// [`render_suite`] is order-independent, either product installs byte-identical
/// content, so both running this is idempotent (no churn). Returns
/// `(skill_name, placements)`.
pub fn install_suite(
    own: &SkillSpec,
    sibling: &SkillSpec,
    home: &Path,
    root: &Path,
    scope: SkillScope,
    dry_run: bool,
) -> std::io::Result<(String, Vec<Placed>)> {
    let (name, content) = render_suite(own, sibling);
    let placed = place_rendered(
        &name,
        &own.version,
        &content,
        home,
        root,
        scope,
        None,
        dry_run,
    )?;
    Ok((name, placed))
}

/// Report each skill-capable host's install state without touching the disk.
pub fn skill_status(
    spec: &SkillSpec,
    home: &Path,
    root: &Path,
    scope: SkillScope,
    tool_filter: Option<&str>,
) -> Vec<(&'static str, SkillState)> {
    let mut out = Vec::new();
    for p in PLATFORMS {
        if let Some(f) = tool_filter {
            if p.key != f {
                continue;
            }
        }
        let Some(skill) = p.skill else { continue };
        let dir = match scope {
            SkillScope::Home => skill.home_dir_under(&spec.slug, home),
            SkillScope::Project => match skill.project_dir(&spec.slug, root) {
                Some(d) => d,
                None => continue,
            },
        };
        out.push((p.key, skill_state(&dir, &spec.version)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skillgen::{STAMP_FILE, example_spec};

    #[test]
    fn places_and_stamps_home_scoped() {
        let tmp = std::env::temp_dir().join(format!("skgen-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let spec = example_spec(); // slug "demo"
        let placed = place_skills(&spec, &tmp, &tmp, SkillScope::Home, None, false, false).unwrap();
        assert!(!placed.is_empty());
        let claude = tmp.join(".claude/skills/demo/SKILL.md");
        assert!(claude.exists());
        assert!(tmp.join(".claude/skills/demo").join(STAMP_FILE).exists());
        assert!(placed.iter().all(|p| p.action == Action::Wrote));
        // all current on re-check
        let states = skill_status(&spec, &tmp, &tmp, SkillScope::Home, None);
        assert!(
            states
                .iter()
                .all(|(_, s)| matches!(s, SkillState::Current { .. }))
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn refuses_downgrade_and_dry_run_is_inert() {
        let tmp = std::env::temp_dir().join(format!("skgen-install2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let spec = example_spec();
        // dry-run writes nothing
        let dry = place_skills(
            &spec,
            &tmp,
            &tmp,
            SkillScope::Home,
            Some("claude-code"),
            false,
            true,
        )
        .unwrap();
        assert_eq!(dry[0].action, Action::WouldWrite);
        assert!(!tmp.join(".claude/skills/demo/SKILL.md").exists());
        // pre-stage a newer install → refused
        let dir = tmp.join(".claude/skills/demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "NEWER").unwrap();
        crate::skillgen::state::write_stamp(&dir, "999.0.0").unwrap();
        let placed = place_skills(
            &spec,
            &tmp,
            &tmp,
            SkillScope::Home,
            Some("claude-code"),
            false,
            false,
        )
        .unwrap();
        assert_eq!(placed[0].action, Action::SkippedNewer);
        assert_eq!(
            std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "NEWER"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
