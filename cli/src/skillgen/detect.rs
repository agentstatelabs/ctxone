//! Cross-product detection + one-time nudge bookkeeping (suite-onboarding
//! t-004). Both ASD and CTX use these to detect whether the sibling is
//! installed and to show the "add the other" prompt exactly once.

use std::path::{Path, PathBuf};

/// Is an executable named `name` on the `PATH`?
pub fn binary_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(name);
        is_executable(&p) || is_executable(&p.with_extension("exe"))
    })
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Marker file recording that the one-time nudge for `sibling_key` was shown.
fn nudge_marker(state_dir: &Path, sibling_key: &str) -> PathBuf {
    state_dir.join(format!(".nudged-{sibling_key}"))
}

/// Has the one-time nudge about `sibling_key` already been shown?
pub fn already_nudged(state_dir: &Path, sibling_key: &str) -> bool {
    nudge_marker(state_dir, sibling_key).exists()
}

/// Record that the one-time nudge about `sibling_key` has now been shown.
pub fn record_nudge(state_dir: &Path, sibling_key: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(nudge_marker(state_dir, sibling_key), b"")
}

/// The pure decision: show the one-time sibling nudge only when the sibling is
/// absent, we haven't shown it before, and it isn't suppressed. Never blocks —
/// this only gates a printed suggestion.
pub fn should_nudge(sibling_present: bool, already_nudged: bool, suppressed: bool) -> bool {
    !sibling_present && !already_nudged && !suppressed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_table() {
        // absent + fresh + allowed → nudge
        assert!(should_nudge(false, false, false));
        // sibling present → never
        assert!(!should_nudge(true, false, false));
        // already nudged → never
        assert!(!should_nudge(false, true, false));
        // suppressed → never
        assert!(!should_nudge(false, false, true));
    }

    #[test]
    fn nudge_marker_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("skgen-nudge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!already_nudged(&tmp, "ctx"));
        record_nudge(&tmp, "ctx").unwrap();
        assert!(already_nudged(&tmp, "ctx"));
        // distinct siblings tracked separately
        assert!(!already_nudged(&tmp, "asd"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detects_a_known_binary() {
        // `sh` is on PATH in every unix CI/dev environment.
        #[cfg(unix)]
        assert!(binary_on_path("sh"));
        assert!(!binary_on_path("definitely-not-a-real-binary-xyz"));
    }
}
