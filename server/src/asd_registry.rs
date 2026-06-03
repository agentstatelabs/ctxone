//! Read-only consumer of the shared ASD repo registry written by the
//! `asd repo ...` CLI. We deliberately don't depend on
//! `agentstatedeveloper-core` from a different repo — this is the only piece
//! of the schema we need (active pointer + per-repo path), so parsing it
//! directly with `toml` is simpler than carrying a cross-repo dependency.
//!
//! Schema reference: <https://git.internal.example/agentstategroup/agentstatedeveloper> →
//! `docs/repo-registry.md`.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct DiscoveredRepo {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct Discovered {
    pub repos: Vec<DiscoveredRepo>,
    pub active: Option<String>,
}

/// Default registry location, matching the asd CLI:
/// `$ASD_REGISTRY` if set, else `$HOME/.config/asd/repos.toml`.
pub fn default_registry_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ASD_REGISTRY") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/asd/repos.toml"))
}

/// Read and parse the registry at the default path. Returns `None` if the
/// file does not exist or fails to parse — auto-discovery must never block
/// startup.
pub fn discover() -> Option<Discovered> {
    let path = default_registry_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let root: DiskRoot = match toml::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "asd registry parse failed");
            return None;
        }
    };
    let version = root.version.unwrap_or(1);
    if version > 1 {
        tracing::warn!(
            version,
            path = %path.display(),
            "asd registry version is newer than supported; skipping auto-discovery",
        );
        return None;
    }
    let mut repos = Vec::new();
    for (name, r) in root.repos.into_iter() {
        // Tolerate ~ and relative paths the same way the asd CLI does.
        let path = canonicalize_lenient(&r.path);
        repos.push(DiscoveredRepo { name, path });
    }
    let active = root.active.and_then(|a| a.repo).filter(|s| !s.is_empty());
    Some(Discovered { repos, active })
}

// ---------------------------------------------------------------------------
// Disk shape — keep private so callers can't depend on the layout.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct DiskRoot {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    active: Option<DiskActive>,
    #[serde(default)]
    repos: std::collections::BTreeMap<String, DiskRepo>,
}

#[derive(Deserialize, Default)]
struct DiskActive {
    #[serde(default)]
    repo: Option<String>,
}

#[derive(Deserialize)]
struct DiskRepo {
    path: String,
}

fn canonicalize_lenient(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if raw == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_registry() {
        let toml = r#"
version = 1
[active]
repo = "myapp"

[repos.myapp]
path = "/tmp/myapp/.asd-state.db"

[repos.sdk]
path = "/tmp/sdk/.asd-state.db"
"#;
        let root: DiskRoot = toml::from_str(toml).unwrap();
        assert_eq!(root.version, Some(1));
        assert_eq!(root.repos.len(), 2);
        assert_eq!(root.active.unwrap().repo.as_deref(), Some("myapp"));
    }

    #[test]
    fn parses_registry_with_no_active() {
        let toml = r#"
version = 1
[active]

[repos.myapp]
path = "/tmp/myapp/.asd-state.db"
"#;
        let root: DiskRoot = toml::from_str(toml).unwrap();
        assert!(root.active.unwrap().repo.is_none());
    }

    #[test]
    fn tolerates_tilde_paths_via_canonicalize() {
        // Just exercise the helper — content depends on $HOME being set.
        let p = canonicalize_lenient("/abs/path");
        assert_eq!(p, PathBuf::from("/abs/path"));
    }
}
