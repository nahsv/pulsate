//! Production HTTPS transport for the ACME client, built on the same hyper +
//! rustls stack the data plane uses.
//!
//! This is the concrete [`AcmeTransport`] for talking to a real CA (Let's
//! Encrypt and friends). The protocol logic lives in [`crate::AcmeClient`]; this
//! module only does HTTP I/O and lifts the headers the client cares about
//! (`Replay-Nonce`, `Location`, `Retry-After`) out of the response.
//!
//! Trust anchors come from the bundled webpki roots, so this validates against
//! the public Web PKI. Testing against a local CA with a private root (Pebble)
//! needs a transport variant with custom roots — see `docs/ROADMAP.md`.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use pulsate_core::{Code, PulsateError, Result};

use crate::client::{AcmeTransport, HttpResponse, Method};

/// `Content-Type` every ACME POST must carry (RFC 8555 §6.2).
const JOSE_CONTENT_TYPE: &str = "application/jose+json";

fn net_err(msg: impl Into<String>) -> PulsateError {
    PulsateError::new(Code::CFG_ACME_UNREACHABLE, msg)
}

/// An HTTPS transport for ACME over hyper + rustls, validating against the
/// public Web PKI.
#[derive(Clone)]
pub struct HttpsTransport {
    client: Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl HttpsTransport {
    /// Build a transport with the bundled webpki trust roots.
    ///
    /// # Errors
    /// Returns an error if the TLS connector cannot be constructed.
    pub fn new() -> Result<Self> {
        let https = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_only()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new()).build(https);
        Ok(Self { client })
    }
}

/// Lift the ACME-relevant headers out of a response map. Pure so it can be
/// unit-tested without a network.
fn extract_headers(status: u16, headers: &http::HeaderMap) -> HttpResponse {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    HttpResponse {
        status,
        replay_nonce: get("replay-nonce"),
        location: get("location"),
        retry_after: get("retry-after").and_then(|v| v.parse::<u64>().ok()),
        body: Vec::new(),
    }
}

fn hyper_method(method: Method) -> hyper::Method {
    match method {
        Method::Get => hyper::Method::GET,
        Method::Head => hyper::Method::HEAD,
        Method::Post => hyper::Method::POST,
    }
}

impl AcmeTransport for HttpsTransport {
    async fn execute(
        &self,
        method: Method,
        url: &str,
        body: Option<String>,
    ) -> Result<HttpResponse> {
        let mut builder = Request::builder().method(hyper_method(method)).uri(url);
        if method == Method::Post {
            builder = builder.header(hyper::header::CONTENT_TYPE, JOSE_CONTENT_TYPE);
        }
        let request = builder
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .map_err(|e| net_err(format!("malformed ACME request: {e}")))?;

        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| net_err(format!("ACME request to {url} failed: {e}")))?;

        // Lift headers (owned) before consuming the body.
        let mut out = extract_headers(response.status().as_u16(), response.headers());
        let collected = response
            .into_body()
            .collect()
            .await
            .map_err(|e| net_err(format!("reading ACME response body failed: {e}")))?;
        out.body = collected.to_bytes().to_vec();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    #[test]
    fn extract_headers_lifts_acme_fields() {
        let mut h = HeaderMap::new();
        h.insert("replay-nonce", "abc123".parse().unwrap());
        h.insert("location", "https://ca/acct/9".parse().unwrap());
        h.insert("retry-after", "30".parse().unwrap());
        let r = extract_headers(201, &h);
        assert_eq!(r.status, 201);
        assert_eq!(r.replay_nonce.as_deref(), Some("abc123"));
        assert_eq!(r.location.as_deref(), Some("https://ca/acct/9"));
        assert_eq!(r.retry_after, Some(30));
    }

    #[test]
    fn extract_headers_tolerates_missing_and_nonnumeric() {
        let h = HeaderMap::new();
        let r = extract_headers(200, &h);
        assert!(r.replay_nonce.is_none());
        assert!(r.location.is_none());
        assert!(r.retry_after.is_none());

        let mut h2 = HeaderMap::new();
        h2.insert(
            "retry-after",
            "Mon, 01 Jan 2030 00:00:00 GMT".parse().unwrap(),
        );
        // HTTP-date Retry-After isn't a bare integer; we surface None rather than
        // misparse it (callers fall back to their own poll interval).
        assert_eq!(extract_headers(200, &h2).retry_after, None);
    }

    #[test]
    fn methods_map_to_hyper() {
        assert_eq!(hyper_method(Method::Get), hyper::Method::GET);
        assert_eq!(hyper_method(Method::Head), hyper::Method::HEAD);
        assert_eq!(hyper_method(Method::Post), hyper::Method::POST);
    }

    #[test]
    fn transport_builds() {
        // The connector/client construct without a network.
        assert!(HttpsTransport::new().is_ok());
    }
}
