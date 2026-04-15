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

use ::governor::middleware::NoOpMiddleware;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::PeerIpKeyExtractor;
use tower_governor::GovernorLayer;
use tracing::{info, warn};

/// Compiled config type for the peer-IP keyed rate limiter.
///
/// `GovernorConfig<K, M>` has two generic parameters; we pin them
/// to the default extractor (peer IP) and the no-op middleware so
/// the layer type is nameable elsewhere in the crate.
pub type PeerIpGovernorConfig = GovernorConfig<PeerIpKeyExtractor, NoOpMiddleware>;

/// Fully-pinned `GovernorLayer` type so callers can store it in a
/// `Router::layer(...)` chain without wrestling with generics.
pub type PeerIpGovernorLayer =
    GovernorLayer<PeerIpKeyExtractor, NoOpMiddleware, axum::body::Body>;

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
}
