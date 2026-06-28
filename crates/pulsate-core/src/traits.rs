//! The capability traits that define Pulsate's extension seams.
//!
//! Every extension point — built-in or plugin — is one of these traits plus a
//! `Registry` registration (`docs/02-architecture.md#extension-points`). They
//! live in `pulsate-core` so the data plane, the SDK, and embedders all depend on
//! the same vocabulary. Concrete implementations live in their own crates.

use std::future::Future;
use std::pin::Pin;

use crate::context::RequestCtx;
use crate::error::Result;
use crate::message::Response;

/// A boxed, `Send` future — the return shape for object-safe async trait methods
/// on the hot path until native async-in-trait dyn dispatch is stable for our MSRV.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The outcome of a middleware's Ingress phase: continue down the pipeline, or
/// short-circuit straight to Egress with the response already on the context
/// (e.g. a rate limiter returning 429).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Flow {
    /// Proceed to the next stage/middleware.
    Continue,
    /// Stop advancing; begin Egress with the current response.
    ShortCircuit,
}

/// A single, composable unit of the request pipeline.
///
/// The same contract unifies built-in, native, and plugin middleware.
/// `on_request` runs in declared order at Ingress;
/// `on_response` runs in reverse order at Egress; `on_error` participates in
/// Recover. Default `on_*` impls make a middleware opt into only the phases it
/// needs.
pub trait Middleware: Send + Sync {
    /// A stable name for diagnostics and ordering.
    fn name(&self) -> &str;

    /// Ingress phase. Default: pass through.
    fn on_request<'a>(&'a self, _ctx: &'a mut RequestCtx) -> BoxFuture<'a, Result<Flow>> {
        Box::pin(async { Ok(Flow::Continue) })
    }

    /// Egress phase. Default: pass through.
    fn on_response<'a>(&'a self, _ctx: &'a mut RequestCtx) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// A terminal request handler chosen at [`crate::lifecycle::Stage::Dispatch`]
/// — a reverse proxy, a static file server, a redirect, or a fixed response. A
/// route has exactly one.
pub trait Handler: Send + Sync {
    /// A stable name for diagnostics.
    fn name(&self) -> &str;

    /// Produce the response for this request. Implementations set the response
    /// on the context and/or return it.
    fn handle<'a>(&'a self, ctx: &'a mut RequestCtx) -> BoxFuture<'a, Result<Response>>;
}

/// A routing predicate evaluated at [`crate::lifecycle::Stage::Match`] (host,
/// path, method, header, …). Matching must be allocation-free and deterministic.
pub trait Matcher: Send + Sync {
    /// Whether this matcher accepts the request in `ctx`.
    fn matches(&self, ctx: &RequestCtx) -> bool;
}

/// A resolved upstream target the proxy can dispatch to. Pools, health, and LB
/// live in `pulsate-proxy`; this trait is the seam discovery plugins implement.
pub trait Upstream: Send + Sync {
    /// A stable identifier (e.g. the `@name` from config).
    fn id(&self) -> &str;
}

/// A pluggable HTTP cache backend (memory/disk/redis). Implemented in
/// `pulsate-cache`; the trait lives here so the pipeline can hold `Arc<dyn CacheStore>`.
pub trait CacheStore: Send + Sync {
    /// A stable backend name (`memory`, `disk`, `redis`).
    fn backend(&self) -> &str;
}

/// A backend that resolves secret references (env/file/Vault/cloud KMS).
pub trait SecretsBackend: Send + Sync {
    /// A stable backend name.
    fn backend(&self) -> &str;

    /// Resolve a secret reference to its value.
    fn resolve<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<String>>;
}

/// A store of TLS certificates, queried by SNI during the handshake and written
/// to by the ACME manager. Implemented in `pulsate-acme`.
pub trait CertStore: Send + Sync {
    /// Whether a certificate is available for the given SNI server name.
    fn has_cert(&self, server_name: &str) -> bool;
}

/// A sink for metric samples emitted by the data plane. Kept non-blocking and
/// bounded-cardinality; implemented in `pulsate-observe`.
pub trait MetricsSink: Send + Sync {
    /// Increment a named counter by `value` with the given label pairs.
    fn incr(&self, name: &str, value: u64, labels: &[(&str, &str)]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RequestId;
    use crate::message::{Body, Request};
    use crate::snapshot::{ConfigSnapshot, SnapshotId};
    use http::{Method, StatusCode, Uri, Version};

    // A trivial Handler proves the trait is object-safe and the boxed-future
    // signature is usable.
    struct Ok200;
    impl Handler for Ok200 {
        #[allow(clippy::unnecessary_literal_bound)] // matches the trait signature
        fn name(&self) -> &str {
            "ok200"
        }
        fn handle<'a>(&'a self, _ctx: &'a mut RequestCtx) -> BoxFuture<'a, Result<Response>> {
            Box::pin(async { Ok(Response::new(StatusCode::OK)) })
        }
    }

    #[test]
    fn handler_is_object_safe() {
        // Proves the trait is object-safe and the boxed-future signature builds.
        // Execution belongs to integration tests once `pulsate-rt` is wired in.
        let h: Box<dyn Handler> = Box::new(Ok200);
        assert_eq!(h.name(), "ok200");

        let req = Request::new(
            Method::GET,
            Uri::from_static("/"),
            Version::HTTP_11,
            http::HeaderMap::new(),
            Body::Empty,
        );
        let snap = ConfigSnapshot::builder(SnapshotId::from_digest(1), 1).build();
        let mut ctx = RequestCtx::new(req, snap, RequestId::new("01J"));

        // Building the future must type-check; we drop it without polling.
        let _fut = h.handle(&mut ctx);
    }
}
