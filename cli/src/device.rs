//! This install's device id (ctx-hub-sync t-001): 32 hex digits, minted once
//! and kept in `~/.ctxone/device_id`, so what reaches a Hub can say which
//! machine it came from. `ctx init` mints it, and so does anything that needs
//! it first.
//!
//! Each session's provenance carries it. Commits will once AgentStateGraph
//! can take it with each call (t-010); plan commits are made inside
//! AgentStateGraph today, with only an agent id.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// This install's device id, minted if it has none yet. `None` when it can't
/// be read or kept: no home directory, or a `device_id` file that isn't one.
pub fn device_id() -> Option<String> {
    device_id_at(&path()?)
}

/// Where the device id is kept.
pub fn path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ctxone").join("device_id"))
}

/// The device id kept at `path`, minted there if there's none.
pub fn device_id_at(path: &Path) -> Option<String> {
    if path.exists() {
        return read(path);
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(format!("{id}\n").as_bytes()).ok()?;
            Some(id)
        }
        // Another `ctx` minted one first: that's the id.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read(path),
        Err(_) => None,
    }
}

/// The id in a `device_id` file, if it holds one.
fn read(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let id = text.trim();
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_once_then_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".ctxone").join("device_id");
        let first = device_id_at(&path).expect("minted");
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(device_id_at(&path).as_deref(), Some(first.as_str()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn two_installs_differ() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert_ne!(
            device_id_at(&a.path().join("device_id")),
            device_id_at(&b.path().join("device_id"))
        );
    }

    #[test]
    fn a_file_that_isnt_an_id_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device_id");
        std::fs::write(&path, "not an id\n").unwrap();
        assert_eq!(device_id_at(&path), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not an id\n");
    }
}
