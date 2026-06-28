//! The normalized HTTP message model.
//!
//! [`Request`] and [`Response`] are protocol-agnostic: the H1/H2/H3 codecs
//! normalize into them so the router, pipeline, and handlers are written once
//! against a single shape (`docs/05-http-stack.md`).
//! Bodies use [`Body`], which is reference-counted ([`bytes::Bytes`]) so clones
//! and slices are zero-copy.

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode, Uri, Version};

/// A response/request body.
///
/// Models the two terminal shapes: empty and fully-buffered `Bytes`. A streaming
/// variant (backpressured chunks for the `Stream` stage) does not exist yet;
/// [`Body`] is `#[non_exhaustive]` so it can grow without a breaking change.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum Body {
    /// No body.
    #[default]
    Empty,
    /// A complete in-memory body.
    Bytes(Bytes),
}

impl Body {
    /// Number of bytes if the length is known up front.
    #[must_use]
    pub fn len_hint(&self) -> Option<usize> {
        match self {
            Body::Empty => Some(0),
            Body::Bytes(b) => Some(b.len()),
        }
    }

    /// Whether the body is known to be empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self.len_hint(), Some(0))
    }
}

impl From<Bytes> for Body {
    fn from(b: Bytes) -> Self {
        if b.is_empty() {
            Body::Empty
        } else {
            Body::Bytes(b)
        }
    }
}

impl From<&'static str> for Body {
    fn from(s: &'static str) -> Self {
        Body::from(Bytes::from_static(s.as_bytes()))
    }
}

impl From<String> for Body {
    fn from(s: String) -> Self {
        Body::from(Bytes::from(s.into_bytes()))
    }
}

/// A normalized inbound request.
#[derive(Debug)]
pub struct Request {
    method: Method,
    uri: Uri,
    version: Version,
    headers: HeaderMap,
    body: Body,
}

impl Request {
    /// Build a request from its parts.
    #[must_use]
    pub fn new(method: Method, uri: Uri, version: Version, headers: HeaderMap, body: Body) -> Self {
        Self {
            method,
            uri,
            version,
            headers,
            body,
        }
    }

    /// The request method.
    #[must_use]
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// The request target URI.
    #[must_use]
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    /// The negotiated HTTP version.
    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    /// The request headers.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Mutable access to the request headers (for Ingress middleware).
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    /// The request body.
    #[must_use]
    pub fn body(&self) -> &Body {
        &self.body
    }
}

/// A response under construction or ready to stream.
#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    version: Version,
    headers: HeaderMap,
    body: Body,
}

impl Response {
    /// A response with the given status and an empty body.
    #[must_use]
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            version: Version::HTTP_11,
            headers: HeaderMap::new(),
            body: Body::Empty,
        }
    }

    /// Set the body, returning `self` for chaining.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<Body>) -> Self {
        self.body = body.into();
        self
    }

    /// The response status.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The HTTP version the response will be written with.
    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    /// The response headers.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Mutable access to the response headers (for Egress middleware).
    pub fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    /// The response body.
    #[must_use]
    pub fn body(&self) -> &Body {
        &self.body
    }
}

impl Default for Response {
    fn default() -> Self {
        Response::new(StatusCode::OK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_from_str_tracks_length_and_emptiness() {
        assert!(Body::from("").is_empty());
        assert_eq!(Body::from("hello").len_hint(), Some(5));
    }

    #[test]
    fn response_builder_sets_status_and_body() {
        let r = Response::new(StatusCode::NOT_FOUND).with_body("nope");
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert_eq!(r.body().len_hint(), Some(4));
    }
}
