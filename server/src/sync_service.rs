//! The Hub serves AgentStateGraph's sync protocol (ctx-hub-sync t-015).
//!
//! With `--serve-sync`, `ctxone-hub` mounts AgentStateGraph's `/sync/v1`
//! router beside its API, behind the same auth: other stores push to a
//! tenant's store and fetch from it at `/sync/v1/<store>/{refs,fetch,push}`.
//! The receiving side's checks are AgentStateGraph's own: a push is a
//! fast-forward of a ref, `/_meta/_secret` and local-only refs are refused,
//! and the control ref's taints, reminders and epochs are merged by its
//! rules.
//!
//! **What `<store>` names.** Each tenant has one store, holding every
//! workspace (an AgentStateGraph namespace) and the control ref; the refs a
//! sync moves carry their namespace. So `<store>` names the tenant: its name
//! on a multi-tenant Hub, `default` on a single-tenant one. [`OWN_STORE`],
//! `me`, always means the caller's own, so a client needn't know its tenant's
//! name. A token never reaches another tenant's store.

use std::sync::Arc;

use agentstategraph::sync::BundleLimits;
use agentstategraph_sync_http::{Rejection, Resolved, StoreResolver};
use axum::Router;
use axum::http::StatusCode;
use axum::http::request::Parts;

use crate::tenant_cache::Tenancy;

/// The store name that always means the caller's own store.
pub const OWN_STORE: &str = "me";

/// What a single-tenant Hub calls its one store.
pub const SINGLE_STORE: &str = "default";

/// The header a client may name its device in, for the push log.
pub const DEVICE_HEADER: &str = "x-ctxone-device";

/// Resolves each sync request to its tenant's store.
pub(crate) struct HubSyncResolver {
    tenancy: Tenancy,
}

impl StoreResolver for HubSyncResolver {
    async fn resolve(&self, store: &str, request: &Parts) -> Result<Resolved, Rejection> {
        // `require_auth` ran first: a multi-tenant Hub's request carries its
        // tenant, and a single-tenant one's passed the token check.
        let (tenant, data) =
            self.tenancy
                .state_for(&request.extensions)
                .await
                .map_err(|(status, message)| match status {
                    StatusCode::SERVICE_UNAVAILABLE => Rejection::NotFound(message.to_string()),
                    _ => Rejection::Unauthorized(message.to_string()),
                })?;
        let own = tenant.as_ref().map_or(SINGLE_STORE, |t| t.as_str());
        if store != OWN_STORE && store != own {
            return Err(Rejection::Forbidden(format!(
                "this token's store is {own:?} (or {OWN_STORE:?}), not {store:?}"
            )));
        }
        if request.uri.path().ends_with("/push") {
            // A push changes the store under the session statistics' cached
            // sizes.
            data.sessions.mark_all_dirty();
        }
        Ok(Resolved {
            repo: data.repo.clone(),
            pusher: pusher(own, request),
        })
    }
}

/// Who the push log records a push under: the tenant, and the device the
/// client names, if it's a plausible id.
fn pusher(own: &str, request: &Parts) -> String {
    let device = request
        .headers
        .get(DEVICE_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|d| {
            !d.is_empty()
                && d.len() <= 64
                && d.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        });
    match device {
        Some(device) => format!("{own}@{device}"),
        None => own.to_string(),
    }
}

/// The `/sync/v1` routes, serving each request's own store, with
/// `max_push_bytes` (AgentStateGraph's default when `None`) as the most one
/// push may send. Merge them inside the auth layer.
pub(crate) fn router(tenancy: Tenancy, max_push_bytes: Option<u64>) -> Router {
    let mut limits = BundleLimits::default();
    if let Some(max) = max_push_bytes {
        limits.max_bytes = max;
    }
    agentstategraph_sync_http::router_with_limits(Arc::new(HubSyncResolver { tenancy }), limits)
}
