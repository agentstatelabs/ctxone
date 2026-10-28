//! Rate limiting for the Hub HTTP API: per tenant on a multi-tenant Hub,
//! per peer IP otherwise.
//!
//! Backed by `tower_governor` (which is itself backed by the `governor`
//! crate). Token-bucket scheme, keyed by the request's tenant or peer IP.
//! When a client exceeds their quota the layer returns `429 Too Many
//! Requests` with `X-RateLimit-*` headers so well-behaved clients back off.
//!
//! ## Per tenant or per IP
//!
//! A multi-tenant Hub knows every request's tenant (the auth middleware
//! attached it), so each tenant gets one bucket wherever its requests come
//! from, and there's no loopback exemption: behind a tunnel or proxy on
//! 127.0.0.1, every request is loopback. A single-tenant Hub has no identity
//! to key on beyond the peer IP, which protects it from runaway loops and
//! simple abuse.
//!
//! ## Loopback
//!
//! On a single-tenant Hub, requests from the same machine — local agents, the
//! `ctx` CLI, the Lens dev proxy, and `/api/sessions/sync` (which fires
//! thousands of loopback POSTs via the spawned CLI) — are trusted and must not
//! be throttled: a rate-limited sync silently drops writes. They skip the
//! limiter ([`RateLimitLayer`]), so they leave nothing in it.
//!
//! ## Idle buckets
//!
//! The limiter keeps a bucket for every key it has seen and never drops one by
//! itself. A thread drops the buckets that have refilled every
//! [`CLEANUP_EVERY`], so an address that stopped calling doesn't stay in
//! memory.
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
//! The peer address comes from axum's `ConnectInfo<SocketAddr>` extension.
//! This means `axum::serve` must be called via
//! `into_make_service_with_connect_info::<SocketAddr>()` for rate limiting to
//! actually see real peer IPs. The Hub's `main.rs` already does this.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use ::governor::middleware::NoOpMiddleware;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, Response};
use tower::{Layer, Service, ServiceExt};
use tower_governor::GovernorLayer;
use tower_governor::errors::GovernorError;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder, SharedRateLimiter};
use tower_governor::key_extractor::KeyExtractor;
use tracing::{debug, info, warn};

use crate::tenant::Tenant;

/// How often the limiter drops the buckets that have refilled. At the
/// default limit a bucket refills 6 seconds after its last request.
pub const CLEANUP_EVERY: Duration = Duration::from_secs(60);

/// Rate-limit key extractor: the request's [`Tenant`] on a multi-tenant Hub,
/// its peer IP otherwise, so each address shares one bucket.
#[derive(Clone)]
pub struct HubKeyExtractor {
    by_tenant: bool,
}

impl KeyExtractor for HubKeyExtractor {
    type Key = String;

    #[cfg(feature = "tracing")]
    fn name(&self) -> &'static str {
        "tenant or peer IP"
    }

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        if self.by_tenant {
            // Auth runs first and attaches the tenant; without one, fail closed.
            return req
                .extensions()
                .get::<Tenant>()
                .map(|t| format!("tenant:{t}"))
                .ok_or(GovernorError::UnableToExtractKey);
        }
        peer(req)
            .map(|addr| addr.ip().to_string())
            .ok_or(GovernorError::UnableToExtractKey)
    }

    #[cfg(feature = "tracing")]
    fn key_name(&self, key: &Self::Key) -> Option<String> {
        Some(key.clone())
    }
}

/// The peer address `into_make_service_with_connect_info` attached, if any.
fn peer<T>(req: &Request<T>) -> Option<SocketAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0)
}

/// Compiled config type for the Hub's rate limiter.
pub type HubGovernorConfig = GovernorConfig<HubKeyExtractor, NoOpMiddleware>;

type HubGovernorLayer = GovernorLayer<HubKeyExtractor, NoOpMiddleware, Body>;

/// The Hub's rate limiter: a `tower_governor` layer, except that on a
/// single-tenant Hub a loopback request skips it and leaves no bucket.
#[derive(Clone)]
pub struct RateLimitLayer {
    governor: HubGovernorLayer,
    config: Arc<HubGovernorConfig>,
    by_tenant: bool,
}

impl RateLimitLayer {
    /// How many buckets the limiter holds.
    pub fn buckets(&self) -> usize {
        self.config.limiter().len()
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimited<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimited {
            inner,
            governor: self.governor.clone(),
            loopback_exempt: !self.by_tenant,
        }
    }
}

/// The service [`RateLimitLayer`] wraps around a route.
#[derive(Clone)]
pub struct RateLimited<S> {
    inner: S,
    governor: HubGovernorLayer,
    loopback_exempt: bool,
}

impl<S> Service<Request<Body>> for RateLimited<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Each call waits for its own clone of `inner`.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let inner = self.inner.clone();
        if self.loopback_exempt && peer(&req).is_some_and(|addr| addr.ip().is_loopback()) {
            return Box::pin(inner.oneshot(req));
        }
        Box::pin(self.governor.layer(inner).oneshot(req))
    }
}

/// Build the rate limiter for `rpm` requests per minute per tenant
/// (`by_tenant`) or per peer IP, or `None` when rate limiting is disabled
/// (rpm = 0). Starts the thread that drops its idle buckets.
pub fn build_layer(rpm: u32, by_tenant: bool) -> Option<RateLimitLayer> {
    let config = Arc::new(build_config(rpm, by_tenant)?);
    spawn_cleanup(config.limiter(), CLEANUP_EVERY);
    info!(rpm, by_tenant, "rate limiter layer attached");
    Some(RateLimitLayer {
        governor: GovernorLayer::new(config.clone()),
        config,
        by_tenant,
    })
}

/// Every `every`, drop the buckets of `limiter` that have refilled, which
/// can't change a later decision. Stops once the limiter is dropped with its
/// router.
fn spawn_cleanup(
    limiter: &SharedRateLimiter<String, NoOpMiddleware>,
    every: Duration,
) -> Option<std::thread::JoinHandle<()>> {
    let limiter = Arc::downgrade(limiter);
    let spawned = std::thread::Builder::new()
        .name("rate-limit-cleanup".into())
        .spawn(move || {
            loop {
                std::thread::sleep(every);
                let Some(limiter) = limiter.upgrade() else {
                    return;
                };
                limiter.retain_recent();
                limiter.shrink_to_fit();
                debug!(buckets = limiter.len(), "rate limiter dropped idle buckets");
            }
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(e) => {
            warn!(error = %e, "can't start the rate limiter's cleanup; idle buckets stay");
            None
        }
    }
}

/// Build just the governor config. Split out so tests can verify the
/// math without instantiating a live layer.
pub fn build_config(rpm: u32, by_tenant: bool) -> Option<HubGovernorConfig> {
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
        .key_extractor(HubKeyExtractor { by_tenant })
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
        assert!(build_config(0, false).is_none());
    }

    #[test]
    fn build_config_rpm_60_yields_valid_config() {
        // 60 rpm = 1 request per second
        let _cfg = build_config(60, false).expect("60 rpm should yield a config");
        // The config itself is opaque; we trust a successful build
        // with the computed math is correct, and cover the behavior
        // end-to-end with an integration test against a live server.
    }

    #[test]
    fn build_config_preserves_minimum_burst() {
        // Very low rpm still needs a minimum burst so a single user
        // can send a handful of requests in a row without getting
        // clobbered. 5 is our floor.
        assert!(build_config(1, false).is_some());
        assert!(build_config(10, false).is_some());
        assert!(build_config(100, false).is_some());
    }

    #[test]
    fn build_layer_rpm_zero_is_none() {
        assert!(build_layer(0, false).is_none());
    }

    #[test]
    fn build_layer_non_zero_is_some() {
        assert!(build_layer(300, false).is_some());
    }

    fn req_from(addr: &str) -> Request<()> {
        let mut r = Request::new(());
        r.extensions_mut()
            .insert(ConnectInfo(addr.parse::<SocketAddr>().unwrap()));
        r
    }

    #[test]
    fn each_address_shares_one_bucket() {
        let ex = HubKeyExtractor { by_tenant: false };
        let r1 = ex.extract(&req_from("203.0.113.7:40000")).unwrap();
        let r2 = ex.extract(&req_from("203.0.113.7:40001")).unwrap();
        assert_eq!(r1, "203.0.113.7");
        assert_eq!(r1, r2, "same remote IP must share one bucket");
        // Loopback has a key too; the layer, not the key, exempts it.
        let lo = ex.extract(&req_from("127.0.0.1:5001")).unwrap();
        assert_eq!(lo, "127.0.0.1");
    }

    #[test]
    fn missing_connect_info_errors() {
        let ex = HubKeyExtractor { by_tenant: false };
        assert!(ex.extract(&Request::new(())).is_err());
    }

    #[test]
    fn by_tenant_keys_by_tenant_wherever_the_request_comes_from() {
        let ex = HubKeyExtractor { by_tenant: true };
        let with_tenant = |addr: &str, name: &str| {
            let mut r = req_from(addr);
            r.extensions_mut().insert(Tenant::new(name).unwrap());
            r
        };
        let a1 = ex.extract(&with_tenant("127.0.0.1:5001", "alice")).unwrap();
        let a2 = ex
            .extract(&with_tenant("203.0.113.7:40000", "alice"))
            .unwrap();
        let b = ex.extract(&with_tenant("127.0.0.1:5001", "bob")).unwrap();
        assert_eq!(a1, "tenant:alice");
        assert_eq!(a1, a2, "one tenant shares one bucket, loopback or not");
        assert_ne!(a1, b, "two tenants on one address get their own buckets");
        // No tenant on a multi-tenant Hub: fail closed.
        assert!(ex.extract(&req_from("127.0.0.1:5001")).is_err());
    }

    /// The statuses of `n` requests from `peer`, with `tenant` attached if
    /// given, through `layer` around a handler that answers 200.
    async fn statuses(
        layer: &RateLimitLayer,
        peer: &str,
        tenant: Option<&str>,
        n: usize,
    ) -> Vec<u16> {
        let ok = tower::service_fn(|_req: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::empty()))
        });
        let service = layer.layer(ok);
        let mut out = Vec::new();
        for _ in 0..n {
            let mut req = Request::new(Body::empty());
            req.extensions_mut()
                .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
            if let Some(tenant) = tenant {
                req.extensions_mut().insert(Tenant::new(tenant).unwrap());
            }
            let resp = service.clone().oneshot(req).await.unwrap();
            out.push(resp.status().as_u16());
        }
        out
    }

    #[tokio::test]
    async fn loopback_skips_the_limiter_and_leaves_no_bucket() {
        // 60 a minute: a burst of 6, then one a second.
        let layer = build_layer(60, false).unwrap();
        for peer in ["127.0.0.1:5001", "[::1]:5001"] {
            let codes = statuses(&layer, peer, None, 50).await;
            assert!(codes.iter().all(|&c| c == 200), "{peer}: {codes:?}");
        }
        assert_eq!(layer.buckets(), 0);
        // A remote peer is limited, in one bucket.
        let codes = statuses(&layer, "203.0.113.7:40000", None, 30).await;
        assert!(codes.contains(&200) && codes.contains(&429), "{codes:?}");
        assert_eq!(layer.buckets(), 1);
    }

    #[tokio::test]
    async fn a_multi_tenant_hub_limits_loopback_too() {
        let layer = build_layer(60, true).unwrap();
        let codes = statuses(&layer, "127.0.0.1:5001", Some("alice"), 30).await;
        assert!(codes.contains(&200) && codes.contains(&429), "{codes:?}");
        assert_eq!(layer.buckets(), 1);
    }

    /// Polls `done` every 10 ms for up to 2 seconds.
    fn eventually(done: impl Fn() -> bool) -> bool {
        (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            done()
        })
    }

    #[test]
    fn cleanup_drops_refilled_buckets_and_stops_with_the_limiter() {
        // A bucket of one that refills in 10 ms.
        let config = GovernorConfigBuilder::default()
            .period(Duration::from_millis(10))
            .burst_size(1)
            .key_extractor(HubKeyExtractor { by_tenant: false })
            .finish()
            .unwrap();
        let limiter = config.limiter().clone();
        assert!(limiter.check_key(&"203.0.113.7".to_string()).is_ok());
        assert_eq!(limiter.len(), 1);

        let cleanup = spawn_cleanup(&limiter, Duration::from_millis(20)).unwrap();
        assert!(eventually(|| limiter.is_empty()), "the idle bucket stayed");

        drop(limiter);
        drop(config);
        assert!(
            eventually(|| cleanup.is_finished()),
            "the cleanup outlived its limiter"
        );
    }
}
