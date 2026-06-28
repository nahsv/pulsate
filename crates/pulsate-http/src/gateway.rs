//! The serving context shared across all connections.
//!
//! A [`Gateway`] bundles the immutable routing table, the upstream registry, and
//! the pooled proxy client so every connection task can route and (when the
//! handler is `proxy`) forward without rebuilding state. It is the data-plane
//! handle the supervisor hands to each listener.

use std::sync::Arc;

use pulsate_observe::Telemetry;
use pulsate_proxy::{ProxyClient, Registry};
use pulsate_router::Router;

/// Shared, cheaply-cloned serving state.
#[derive(Clone)]
pub struct Gateway {
    /// The compiled routing table.
    pub router: Arc<Router>,
    /// Named upstream pools for `proxy(@name)`.
    pub upstreams: Arc<Registry>,
    /// The pooled client used to reach upstreams.
    pub client: ProxyClient,
    /// Metrics and access-log sink.
    pub telemetry: Arc<Telemetry>,
    /// `Alt-Svc` header value to advertise (HTTP/3 discovery), if any.
    pub alt_svc: Option<String>,
}

impl Gateway {
    /// Build a gateway from a router and upstream registry, creating a fresh
    /// proxy client and metrics registry.
    #[must_use]
    pub fn new(router: Arc<Router>, upstreams: Arc<Registry>) -> Self {
        Self::with_telemetry(router, upstreams, Arc::new(Telemetry::new()))
    }

    /// Build a gateway with a shared [`Telemetry`] (so the metrics endpoint and
    /// the serving path observe the same registry).
    #[must_use]
    pub fn with_telemetry(
        router: Arc<Router>,
        upstreams: Arc<Registry>,
        telemetry: Arc<Telemetry>,
    ) -> Self {
        Self {
            router,
            upstreams,
            client: ProxyClient::new(),
            telemetry,
            alt_svc: None,
        }
    }

    /// Set the `Alt-Svc` value to advertise on every response.
    #[must_use]
    pub fn with_alt_svc(mut self, alt_svc: Option<String>) -> Self {
        self.alt_svc = alt_svc;
        self
    }
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field("sites", &self.router.site_count())
            .field("upstreams", &self.upstreams.len())
            .finish_non_exhaustive()
    }
}
