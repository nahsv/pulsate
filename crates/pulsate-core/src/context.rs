//! The per-request context threaded through every lifecycle stage.
//!
//! A single [`RequestCtx`] carries the request, the response-in-progress, the
//! snapshot the request is pinned to, a typed extensions map, timing, and the
//! request ID. Middleware and handlers receive `&mut RequestCtx` — it is the
//! injection vehicle on the hot path; there is no ambient global state
//! (`docs/02-architecture.md#dependency-injection-strategy`).

use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::lifecycle::Stage;
use crate::message::{Request, Response};
use crate::snapshot::ConfigSnapshot;

/// A request-scoped, opaque, time-sortable identifier.
///
/// Holds the rendered string (a ULID in production). It correlates the access
/// log, metrics exemplars, traces, and `problem+json` bodies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    /// Wrap an already-rendered identifier.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A typed, request-scoped key/value map.
///
/// Middleware stash and retrieve state by type (e.g. an authenticated principal,
/// a matched-route handle). Keyed by [`TypeId`] so lookups are type-safe and
/// allocation is only paid for what is actually stored.
#[derive(Default)]
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Extensions {
    /// An empty extensions map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value, returning the previous value of the same type if any.
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|prev| prev.downcast::<T>().ok().map(|b| *b))
    }

    /// Borrow a value by type.
    #[must_use]
    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    /// Mutably borrow a value by type.
    pub fn get_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }
}

impl std::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extensions")
            .field("len", &self.map.len())
            .finish()
    }
}

/// The state threaded through all lifecycle stages for one request.
#[derive(Debug)]
pub struct RequestCtx {
    request: Request,
    response: Option<Response>,
    snapshot: ConfigSnapshot,
    request_id: RequestId,
    stage: Stage,
    extensions: Extensions,
}

impl RequestCtx {
    /// Create a context at the [`Stage::Decode`] boundary, pinned to the
    /// snapshot the request will run against for its entire life.
    #[must_use]
    pub fn new(request: Request, snapshot: ConfigSnapshot, request_id: RequestId) -> Self {
        Self {
            request,
            response: None,
            snapshot,
            request_id,
            stage: Stage::Decode,
            extensions: Extensions::new(),
        }
    }

    /// The inbound request.
    #[must_use]
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// Mutable access to the inbound request.
    pub fn request_mut(&mut self) -> &mut Request {
        &mut self.request
    }

    /// The response-in-progress, once a handler or Recover has produced one.
    #[must_use]
    pub fn response(&self) -> Option<&Response> {
        self.response.as_ref()
    }

    /// Install the response produced by a handler or the Recover phase.
    pub fn set_response(&mut self, response: Response) {
        self.response = Some(response);
    }

    /// The immutable snapshot this request is pinned to.
    #[must_use]
    pub fn snapshot(&self) -> &ConfigSnapshot {
        &self.snapshot
    }

    /// The request's correlation ID.
    #[must_use]
    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// The current lifecycle stage.
    #[must_use]
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Advance the recorded lifecycle stage (driven by the pipeline engine).
    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
    }

    /// Shared access to the request-scoped extensions.
    #[must_use]
    pub fn extensions(&self) -> &Extensions {
        &self.extensions
    }

    /// Mutable access to the request-scoped extensions.
    pub fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Body;
    use crate::snapshot::SnapshotId;
    use http::{Method, Uri, Version};

    fn ctx() -> RequestCtx {
        let req = Request::new(
            Method::GET,
            Uri::from_static("/"),
            Version::HTTP_11,
            http::HeaderMap::new(),
            Body::Empty,
        );
        let snap = ConfigSnapshot::builder(SnapshotId::from_digest(1), 1).build();
        RequestCtx::new(req, snap, RequestId::new("01JTESTULID"))
    }

    #[test]
    fn extensions_are_type_keyed() {
        struct Principal(&'static str);
        let mut ext = Extensions::new();
        assert!(ext.insert(Principal("alice")).is_none());
        assert_eq!(ext.get::<Principal>().unwrap().0, "alice");
        // Re-inserting the same type returns the previous value.
        let prev = ext.insert(Principal("bob"));
        assert_eq!(prev.unwrap().0, "alice");
    }

    #[test]
    fn ctx_starts_at_decode_with_no_response() {
        let c = ctx();
        assert_eq!(c.stage(), Stage::Decode);
        assert!(c.response().is_none());
        assert_eq!(c.request_id().as_str(), "01JTESTULID");
    }

    #[test]
    fn setting_a_response_is_observable() {
        let mut c = ctx();
        c.set_response(Response::new(http::StatusCode::OK));
        assert_eq!(c.response().unwrap().status(), http::StatusCode::OK);
    }
}
