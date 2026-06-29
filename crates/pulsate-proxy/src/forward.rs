//! Request forwarding: build the upstream request, send it via a pooled hyper
//! client, retry across targets, and normalize the response.
//!
//! Implements the [Upstream] lifecycle stage (`docs/02-architecture.md`): pick a
//! healthy target, rewrite hop-by-hop headers, add `X-Forwarded-*`/`Via`, and on
//! a connect error or a retryable status try the next target within the retry
//! budget, recording failures so the breaker can eject a bad target.

use std::net::IpAddr;

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use pulsate_core::{Code, Response};

use crate::Upstream;

/// Hop-by-hop headers that must not be forwarded (RFC 9110 §7.6.1) plus `Host`
/// and `Content-Length`, which are set fresh per hop.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

/// A pooled HTTP client used to reach upstreams. Cheap to clone; share one per
/// process so connection pools are reused across requests.
#[derive(Clone)]
pub struct ProxyClient {
    inner: Client<HttpConnector, Full<Bytes>>,
}

impl ProxyClient {
    /// Build a client with hyper's default connection pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Client::builder(TokioExecutor::new()).build_http(),
        }
    }
}

impl Default for ProxyClient {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProxyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProxyClient")
    }
}

/// Forward a request to `upstream`, returning the upstream response (or a synthetic
/// error response on failure). Retries across targets per the upstream's policy.
#[allow(clippy::too_many_arguments)]
pub async fn forward(
    client: &ProxyClient,
    upstream: &Upstream,
    method: &Method,
    path_and_query: &str,
    req_headers: &HeaderMap,
    body: Bytes,
    client_ip: Option<IpAddr>,
    orig_host: &str,
) -> Response {
    let retry = upstream.retry().clone();
    let total_tries = retry.attempts.max(1);
    let mut last_error = error_response(Code::PRX_NO_HEALTHY);

    for _ in 0..total_tries {
        let Some(idx) = upstream.pick(client_ip) else {
            return error_response(Code::PRX_NO_HEALTHY);
        };
        let Some(base) = upstream.target_url(idx).map(ToString::to_string) else {
            return error_response(Code::PRX_NO_HEALTHY);
        };

        let Some(request) = build_request(
            method,
            &base,
            path_and_query,
            req_headers,
            body.clone(),
            client_ip,
            orig_host,
        ) else {
            return error_response(Code::PRX_PROTOCOL_ERROR);
        };

        upstream.inflight_inc(idx);
        let result = client.inner.request(request).await;
        upstream.inflight_dec(idx);

        if let Ok(resp) = result {
            let status = resp.status().as_u16();
            if retry.retry_on_status.contains(&status) {
                upstream.record_failure(idx);
                last_error = error_response(Code::PRX_NO_HEALTHY);
                continue; // try another target
            }
            upstream.record_success(idx);
            return normalize_response(resp).await;
        }

        // Connect/transport error.
        upstream.record_failure(idx);
        last_error = error_response(Code::PRX_CONNECT_TIMEOUT);
        if retry.on_connect_error {
            continue;
        }
        return last_error;
    }

    // Budget exhausted without a usable response.
    let _ = last_error;
    error_response(Code::PRX_RETRY_EXHAUSTED)
}

/// Build the outgoing upstream request with rewritten headers.
fn build_request(
    method: &Method,
    base: &str,
    path_and_query: &str,
    req_headers: &HeaderMap,
    body: Bytes,
    client_ip: Option<IpAddr>,
    orig_host: &str,
) -> Option<hyper::Request<Full<Bytes>>> {
    let uri: http::Uri = format!("{base}{path_and_query}").parse().ok()?;
    let mut builder = hyper::Request::builder().method(method.clone()).uri(uri);

    let headers = builder.headers_mut()?;
    for (name, value) in req_headers {
        if !HOP_BY_HOP.contains(&name.as_str()) {
            headers.insert(name, value.clone());
        }
    }
    apply_forwarded(headers, req_headers, client_ip, orig_host);

    builder.body(Full::new(body)).ok()
}

/// Add/extend `X-Forwarded-*` and `Via` headers.
fn apply_forwarded(
    out: &mut HeaderMap,
    orig: &HeaderMap,
    client_ip: Option<IpAddr>,
    orig_host: &str,
) {
    if let Some(ip) = client_ip {
        let xff = match orig.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            Some(existing) => format!("{existing}, {ip}"),
            None => ip.to_string(),
        };
        if let Ok(v) = HeaderValue::from_str(&xff) {
            out.insert(HeaderName::from_static("x-forwarded-for"), v);
        }
    }
    if let Ok(v) = HeaderValue::from_str(orig_host) {
        out.insert(HeaderName::from_static("x-forwarded-host"), v);
    }
    out.insert(
        HeaderName::from_static("x-forwarded-proto"),
        HeaderValue::from_static("http"),
    );
    out.insert(http::header::VIA, HeaderValue::from_static("1.1 pulsate"));
}

/// Collect the upstream response into a normalized [`Response`], dropping
/// hop-by-hop headers.
async fn normalize_response(resp: hyper::Response<hyper::body::Incoming>) -> Response {
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();

    let mut out = Response::new(status);
    for (name, value) in &headers {
        if !HOP_BY_HOP.contains(&name.as_str()) {
            out.headers_mut().insert(name, value.clone());
        }
    }
    out.with_body(body)
}

/// Build a synthetic error response for a proxy failure code.
fn error_response(code: Code) -> Response {
    let status = StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp = Response::new(status);
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp.with_body(format!("{code}: {}", code.title()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_response_maps_code_to_status() {
        let r = error_response(Code::PRX_NO_HEALTHY);
        assert_eq!(r.status().as_u16(), Code::PRX_NO_HEALTHY.http_status());
    }

    #[test]
    fn forwarded_for_appends_to_existing_chain() {
        let mut orig = HeaderMap::new();
        orig.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.7"));
        let mut out = HeaderMap::new();
        apply_forwarded(&mut out, &orig, Some("198.51.100.2".parse().unwrap()), "h");
        assert_eq!(
            out.get("x-forwarded-for").unwrap(),
            "203.0.113.7, 198.51.100.2"
        );
        assert_eq!(out.get("via").unwrap(), "1.1 pulsate");
    }
}
