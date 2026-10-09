//! Install-state + version safety for an installed skill (suite-onboarding
//! t-003). Both ASD and CTX use these to stamp installs, detect stale/newer
//! skills, and refuse a silent downgrade — the safety graphify gets from its
//! `.graphify_version` handling.

use std::cmp::Ordering;
use std::path::Path;

/// The version-stamp filename written inside a skill directory. The dir is
/// already product-namespaced (e.g. `~/.claude/skills/asd/`), so a plain name
/// is unambiguous.
pub const STAMP_FILE: &str = ".version";

/// Parse a dotted version into numeric components, ignoring a leading `v` and
/// any pre-release/build suffix. `"v1.1.23-beta"` → `[1, 1, 23]`.
fn parse(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|s| s.parse::<u64>().ok())
        .take_while(Option::is_some)
        .flatten()
        .collect()
}

/// Compare two dotted versions numerically (`1.1` == `1.1.0`).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (mut pa, mut pb) = (parse(a), parse(b));
    let n = pa.len().max(pb.len());
    pa.resize(n, 0);
    pb.resize(n, 0);
    pa.cmp(&pb)
}

/// The state of an installed skill relative to the running package version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillState {
    /// No skill directory / `SKILL.md` present.
    NotInstalled,
    /// Directory exists but `SKILL.md` is gone → repair.
    Missing,
    /// `SKILL.md` present but no version stamp → (re)stamp on next install.
    Unstamped,
    /// Installed version matches the package.
    Current { version: String },
    /// Installed is older than the package → update.
    Stale { installed: String, package: String },
    /// Installed is newer than the package → refuse to overwrite (a downgrade).
    Newer { installed: String, package: String },
}

impl SkillState {
    /// True when installing would overwrite a strictly-newer on-disk skill.
    pub fn is_downgrade(&self) -> bool {
        matches!(self, SkillState::Newer { .. })
    }
}

/// Read the version stamp in a skill directory, if present.
pub fn read_stamp(skill_dir: &Path) -> Option<String> {
    std::fs::read_to_string(skill_dir.join(STAMP_FILE))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Write the version stamp into a skill directory.
pub fn write_stamp(skill_dir: &Path, version: &str) -> std::io::Result<()> {
    std::fs::write(skill_dir.join(STAMP_FILE), format!("{}\n", version.trim()))
}

/// Classify the installed skill at `skill_dir` against `package_version`.
pub fn skill_state(skill_dir: &Path, package_version: &str) -> SkillState {
    if !skill_dir.join("SKILL.md").exists() {
        return if skill_dir.exists() {
            SkillState::Missing
        } else {
            SkillState::NotInstalled
        };
    }
    match read_stamp(skill_dir) {
        None => SkillState::Unstamped,
        Some(installed) => match compare_versions(&installed, package_version) {
            Ordering::Less => SkillState::Stale {
                installed,
                package: package_version.to_string(),
            },
            Ordering::Greater => SkillState::Newer {
                installed,
                package: package_version.to_string(),
            },
            Ordering::Equal => SkillState::Current { version: installed },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert_eq!(compare_versions("1.1.0", "1.1"), Ordering::Equal);
        assert_eq!(compare_versions("1.1.23", "1.1.24"), Ordering::Less);
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.3-beta", "1.2.3"), Ordering::Equal);
    }

    #[test]
    fn states() {
        let tmp = std::env::temp_dir().join(format!("skgen-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // NotInstalled
        assert_eq!(skill_state(&tmp, "1.0.0"), SkillState::NotInstalled);
        // Missing: dir exists, no SKILL.md
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(skill_state(&tmp, "1.0.0"), SkillState::Missing);
        // Unstamped: SKILL.md, no .version
        std::fs::write(tmp.join("SKILL.md"), "x").unwrap();
        assert_eq!(skill_state(&tmp, "1.0.0"), SkillState::Unstamped);
        // Stamped comparisons
        write_stamp(&tmp, "1.0.0").unwrap();
        assert!(matches!(
            skill_state(&tmp, "1.0.0"),
            SkillState::Current { .. }
        ));
        assert!(matches!(
            skill_state(&tmp, "1.1.0"),
            SkillState::Stale { .. }
        ));
        let newer = skill_state(&tmp, "0.9.0");
        assert!(newer.is_downgrade(), "got {newer:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
