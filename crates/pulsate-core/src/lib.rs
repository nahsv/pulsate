//! `pulsate-core` — the shared vocabulary of the Pulsate gateway.
//!
//! This crate is the root of the workspace dependency DAG: it depends on nothing
//! internal, and almost everything depends on it (`docs/03-repository.md`). It
//! defines the types and traits every other crate speaks in — the request
//! lifecycle, the message model, the error taxonomy, the configuration snapshot,
//! the per-request context, and the capability traits at every extension seam.
//!
//! Nothing here performs I/O or pulls in a runtime; it is pure vocabulary so the
//! data plane, the control plane, the plugin SDK, and embedders can all share it.
#![forbid(unsafe_code)]

pub mod context;
pub mod error;
pub mod lifecycle;
pub mod message;
pub mod snapshot;
pub mod traits;

#[doc(inline)]
pub use context::{Extensions, RequestCtx, RequestId};
#[doc(inline)]
pub use error::{Category, Code, PulsateError, Result};
#[doc(inline)]
pub use lifecycle::{Lifecycle, Stage};
#[doc(inline)]
pub use message::{Body, Request, Response};
#[doc(inline)]
pub use snapshot::{ConfigSnapshot, SnapshotBuilder, SnapshotId};
#[doc(inline)]
pub use traits::{
    BoxFuture, CacheStore, CertStore, Flow, Handler, Matcher, MetricsSink, Middleware,
    SecretsBackend, Upstream,
};
