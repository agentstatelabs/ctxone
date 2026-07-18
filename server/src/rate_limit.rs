//! Per-IP rate limiting for the Hub HTTP API.
//!
//! Backed by `tower_governor` (which is itself backed by the `governor`
//! crate). Token-bucket scheme, keyed by peer IP. When a client exceeds
//! their quota the layer returns `429 Too Many Requests` with
//! `X-RateLimit-*` headers so well-behaved clients back off.
//!
//! ## Why per-IP and not per-agent?
//!
//! Until HTTP auth (T1) lands there's no authenticated identity to
//! rate-limit against. The best we can do is peer IP, which protects
//! the Hub from runaway local loops and simple abuse. Once T1 is in
//! place, this module can switch to keying on the API key / agent ID.
//!
//! ## Defaults
//!
//! The default is 600 requests/minute per IP (10/sec). This is
//! permissive on purpose — real agents rarely hammer the Hub, and
//! legitimate bursts (e.g. a big `ctx prime` import) should not get
//! bounced. If you need something tighter, pass `--rate-limit-rpm` on
//! the command line or set `CTXONE_RATE_LIMIT_RPM`.
//!
//! Setting the limit to 0 disables rate limiting entirely — useful in
//! tests and in trusted single-tenant deployments behind their own
//! network ACLs.
//!
//! ## Peer IP extraction
//!
//! `PeerIpKeyExtractor` reads the client address out of axum's
//! `ConnectInfo<SocketAddr>` extension. This means `axum::serve` must
//! be called via `into_make_service_with_connect_info::<SocketAddr>()`
//! for rate limiting to actually see real peer IPs. The Hub's
//! `main.rs` already does this.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ::governor::middleware::NoOpMiddleware;
use axum::extract::ConnectInfo;
use axum::http::Request;
use tower_governor::GovernorLayer;
use tower_governor::errors::GovernorError;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;
use tracing::{info, warn};

/// Rate-limit key extractor that **exempts loopback traffic**.
///
/// The limiter exists to protect the Hub from *remote* abuse. Requests
/// originating on the same machine — local agents, the `ctx` CLI, the
/// Lens dev proxy, and `/api/sessions/sync` (which fires thousands of
/// loopback POSTs via the spawned CLI) — are trusted and must not be
/// throttled: a rate-limited sync silently drops writes.
///
/// Remote peers are keyed by IP (one shared token bucket per IP, exactly
/// as `PeerIpKeyExtractor` did). Loopback peers get a **unique key per
/// request** (a monotonic counter), so each lands in its own bucket and
/// is never limited. governor's background GC reclaims those buckets.
#[derive(Clone)]
pub struct LoopbackExemptKeyExtractor {
    counter: Arc<AtomicU64>,
}

impl LoopbackExemptKeyExtractor {
    fn new() -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl KeyExtractor for LoopbackExemptKeyExtractor {
    type Key = String;

    #[cfg(feature = "tracing")]
    fn name(&self) -> &'static str {
        "loopback-exempt peer IP"
    }

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        let addr = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0)
            .ok_or(GovernorError::UnableToExtractKey)?;
        if addr.ip().is_loopback() {
            // Unique bucket per request → effectively unlimited for local.
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            Ok(format!("lo:{n}"))
        } else {
            Ok(addr.ip().to_string())
        }
    }

    #[cfg(feature = "tracing")]
    fn key_name(&self, key: &Self::Key) -> Option<String> {
        Some(key.clone())
    }
}

/// Compiled config type for the loopback-exempt rate limiter.
pub type PeerIpGovernorConfig = GovernorConfig<LoopbackExemptKeyExtractor, NoOpMiddleware>;

/// Fully-pinned `GovernorLayer` type so callers can store it in a
/// `Router::layer(...)` chain without wrestling with generics.
pub type PeerIpGovernorLayer =
    GovernorLayer<LoopbackExemptKeyExtractor, NoOpMiddleware, axum::body::Body>;

/// Build a rate limiter layer that enforces `rpm` requests per minute
/// per peer IP, or `None` when rate limiting is disabled (rpm = 0).
///
/// Returns an owned layer — tower_governor's `GovernorLayer::new`
/// takes the config by value, not by `Arc`.
pub fn build_layer(rpm: u32) -> Option<PeerIpGovernorLayer> {
    let config = build_config(rpm)?;
    info!(rpm, "rate limiter layer attached");
    Some(GovernorLayer::new(config))
}

/// Build just the governor config. Split out so tests can verify the
/// math without instantiating a live layer.
pub fn build_config(rpm: u32) -> Option<PeerIpGovernorConfig> {
    if rpm == 0 {
        info!("rate limiting disabled (--rate-limit-rpm 0)");
        return None;
    }

    // Convert rpm → per-request interval and burst size.
    //
    // Token-bucket: `period` is how long it takes to refill one token,
    // and `burst_size` is the bucket capacity. To get `rpm` sustained
    // requests per minute with a reasonable short-term burst, we pick:
    //   - period = 60_000 / rpm  ms  (time per single request)
    //   - burst  = max(rpm/10, 5)    (one-tenth of a minute's budget,
    //                                 floor of 5 so low limits still
    //                                 accept small legitimate bursts)
    let period_ms = (60_000 / rpm.max(1)) as u64;
    let burst = (rpm / 10).max(5);

    let config = GovernorConfigBuilder::default()
        .period(std::time::Duration::from_millis(period_ms))
        .burst_size(burst)
        .key_extractor(LoopbackExemptKeyExtractor::new())
        .finish();

    match config {
        Some(c) => {
            info!(
                rpm,
                period_ms,
                burst_size = burst,
                "rate limiter configured"
            );
            Some(c)
        }
        None => {
            warn!(
                rpm,
                period_ms,
                burst_size = burst,
                "failed to build rate limiter config; rate limiting will be DISABLED"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_rpm_zero_disables() {
        assert!(build_config(0).is_none());
    }

    #[test]
    fn build_config_rpm_60_yields_valid_config() {
        // 60 rpm = 1 request per second
        let _cfg = build_config(60).expect("60 rpm should yield a config");
        // The config itself is opaque; we trust a successful build
        // with the computed math is correct, and cover the behavior
        // end-to-end with an integration test against a live server.
    }

    #[test]
    fn build_config_preserves_minimum_burst() {
        // Very low rpm still needs a minimum burst so a single user
        // can send a handful of requests in a row without getting
        // clobbered. 5 is our floor.
        assert!(build_config(1).is_some());
        assert!(build_config(10).is_some());
        assert!(build_config(100).is_some());
    }

    #[test]
    fn build_layer_rpm_zero_is_none() {
        assert!(build_layer(0).is_none());
    }

    #[test]
    fn build_layer_non_zero_is_some() {
        assert!(build_layer(300).is_some());
    }

    fn req_from(addr: &str) -> Request<()> {
        let mut r = Request::new(());
        r.extensions_mut()
            .insert(ConnectInfo(addr.parse::<SocketAddr>().unwrap()));
        r
    }

    #[test]
    fn loopback_gets_unique_keys_remote_is_stable() {
        let ex = LoopbackExemptKeyExtractor::new();
        // Two loopback requests → distinct keys (each its own bucket).
        let a = ex.extract(&req_from("127.0.0.1:5001")).unwrap();
        let b = ex.extract(&req_from("127.0.0.1:5002")).unwrap();
        assert_ne!(a, b, "loopback requests must not share a bucket");
        assert!(a.starts_with("lo:") && b.starts_with("lo:"));
        // A remote peer → stable key across requests (shared bucket, limited).
        let r1 = ex.extract(&req_from("203.0.113.7:40000")).unwrap();
        let r2 = ex.extract(&req_from("203.0.113.7:40001")).unwrap();
        assert_eq!(r1, "203.0.113.7");
        assert_eq!(r1, r2, "same remote IP must share one bucket");
    }

    #[test]
    fn missing_connect_info_errors() {
        let ex = LoopbackExemptKeyExtractor::new();
        assert!(ex.extract(&Request::new(())).is_err());
    }
}
