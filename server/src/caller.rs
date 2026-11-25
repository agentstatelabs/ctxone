//! Who a write comes from, as each commit records it: the agent, and the
//! device it runs on (ctx-hub-sync t-016).
//!
//! `ctx` names its device in the `X-CTXone-Device` header on REST, and in the
//! MCP entries `ctx init` writes: a header, or `?device=` for a client that
//! can't send one. A stdio MCP server runs on the caller's own machine, so it
//! reads the device id `ctx` keeps there. The Hub builds an AgentStateGraph
//! [`CallContext`] from these, and every commit and plan write it makes for
//! the caller goes through it. A Hub's own writes (migrations, sync, epoch
//! backfills) carry no device.

use std::path::{Path, PathBuf};

use crate::sync_service::DEVICE_HEADER;
pub use agentstategraph::CallContext;

/// The longest device id the Hub records. `ctx` mints 32 hex digits.
pub const MAX_DEVICE_LEN: usize = 64;

/// The environment variable that names a stdio MCP server's device in place of
/// the one `ctx` keeps in `~/.ctxone/device_id`.
pub const DEVICE_ENV: &str = "CTX_DEVICE_ID";

/// `raw` as a device id, if it's a plausible one: 1 to [`MAX_DEVICE_LEN`]
/// ASCII letters, digits, `-`, `_` or `.`, after trimming. The sync push log
/// accepts the same.
pub fn plausible_device(raw: &str) -> Option<String> {
    let device = raw.trim();
    let plausible = !device.is_empty()
        && device.len() <= MAX_DEVICE_LEN
        && device
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    plausible.then(|| device.to_string())
}

/// The device a request's `X-CTXone-Device` header names, if plausible.
pub fn device_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(DEVICE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(plausible_device)
}

/// This machine's device id, for a stdio MCP server: [`DEVICE_ENV`] if set,
/// else the id `ctx` keeps in `~/.ctxone/device_id`. Never minted here: `None`
/// until `ctx` has made one.
pub fn local_device() -> Option<String> {
    if let Ok(device) = std::env::var(DEVICE_ENV) {
        return plausible_device(&device);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    device_in(&home.join(".ctxone").join("device_id"))
}

/// The device id in a `device_id` file, if it holds a plausible one.
pub fn device_in(path: &Path) -> Option<String> {
    plausible_device(&std::fs::read_to_string(path).ok()?)
}

/// The caller `agent_id`, on `device` if it named one.
pub fn caller(agent_id: impl Into<String>, device: Option<String>) -> CallContext {
    let context = CallContext::new(agent_id);
    match device {
        Some(device) => context.with_device(device),
        None => context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph::CommitOptions;
    use agentstategraph_core::IntentCategory;

    #[test]
    fn plausible_devices_pass_and_others_dont() {
        assert_eq!(
            plausible_device(" 0123456789abcdef0123456789abcdef\n").as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            plausible_device("laptop-2.home_net").as_deref(),
            Some("laptop-2.home_net")
        );
        assert_eq!(plausible_device(""), None);
        assert_eq!(plausible_device("   "), None);
        assert_eq!(plausible_device("has space"), None);
        assert_eq!(plausible_device("tab\there"), None);
        assert_eq!(plausible_device("ümlaut"), None);
        assert_eq!(
            plausible_device(&"a".repeat(MAX_DEVICE_LEN)).map(|d| d.len()),
            Some(64)
        );
        assert_eq!(plausible_device(&"a".repeat(MAX_DEVICE_LEN + 1)), None);
    }

    #[test]
    fn the_header_names_the_device() {
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(device_from_headers(&headers), None);
        headers.insert(DEVICE_HEADER, "abc123".parse().unwrap());
        assert_eq!(device_from_headers(&headers).as_deref(), Some("abc123"));
        headers.insert(DEVICE_HEADER, "not/plausible".parse().unwrap());
        assert_eq!(device_from_headers(&headers), None);
    }

    #[test]
    fn a_device_file_is_read_not_minted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device_id");
        assert_eq!(device_in(&path), None);
        assert!(!path.exists(), "reading must not mint a device id");
        std::fs::write(&path, "0123456789abcdef0123456789abcdef\n").unwrap();
        assert_eq!(
            device_in(&path).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn commits_carry_the_callers_device() {
        let on_laptop = caller("claude-code", Some("laptop".into()));
        let opts = on_laptop.commit_options(IntentCategory::Plan, "plan");
        assert_eq!(opts.agent_id, "claude-code");
        assert_eq!(opts.device.as_deref(), Some("laptop"));

        let nowhere = caller("claude-code", None);
        let opts = nowhere.commit_options(IntentCategory::Plan, "plan");
        assert_eq!(opts.device, None);
        // Same authority as before devices: the agent authorizes its own write.
        let plain = CommitOptions::new("claude-code", IntentCategory::Plan, "plan");
        assert_eq!(opts.authority.principal, plain.authority.principal);
        assert_eq!(opts.authority.scope, plain.authority.scope);
        assert_eq!(
            opts.authority.delegation_chain,
            plain.authority.delegation_chain
        );
    }
}
