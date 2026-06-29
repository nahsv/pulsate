//! Protocol-agnostic request dispatch for the HTTP/3 listener.
//!
//! [`dispatch`] takes a decoded request head plus a fully-buffered body and runs
//! it through the exact same data-plane core the HTTP/1 and HTTP/2 listeners use:
//! route via [`pulsate_router::Router`], run the [`pulsate_pipeline`] middleware
//! (Ingress short-circuit, then Egress), serve from / store to the
//! [`pulsate_cache`] layer, and execute the terminal handler — a static
//! `respond`/`redirect`/`files` via [`pulsate_http::handlers`] or a `proxy`
//! forward via [`pulsate_proxy::forward`]. The result is a normalized
//! [`pulsate_core::Response`]; the QUIC/h3 layer translates it onto the request
//! stream (`crate::server`).
//!
//! This mirrors the private `handle` in `pulsate_http::serve`. That crate exposes
//! no public dispatch seam, and this crate may not modify it, so the orchestration
//! is reproduced here against the same public building blocks — keeping routing,
//! middleware, caching, access logging, and metrics identical across protocols
//! (`docs/02-architecture.md#request-lifecycle`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use pulsate_core::{Body, Response};
use pulsate_http::Gateway;
use pulsate_observe::{request_id, AccessLog};
use pulsate_proxy::{Policy, Upstream};
use pulsate_router::{Handler, Route};

/// Route and execute one request, returning a fully-buffered response, and record
/// metrics + an access-log line ([Finalize], `docs/02-architecture.md`).
///
/// `parts` is the decoded request head, `body` the fully-buffered request body
/// (HTTP/3 streams it in over QUIC; the shared core consumes it as one `Bytes`),
/// and `peer` the client address (used for `X-Forwarded-For` and IP-hash LB).
pub async fn dispatch(
    parts: http::request::Parts,
    body: Bytes,
    peer: SocketAddr,
    gateway: &Gateway,
) -> Response {
    let started = Instant::now();
    let rid = request_id();

    let host = request_host(&parts);
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().map(ToString::to_string);
    let full_target = match &query {
        Some(q) => format!("{path}?{q}"),
        None => path.clone(),
    };
    let method = parts.method.clone();
    let origin = parts
        .headers
        .get(http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);

    // Route by the original path, then run the pipeline (which may rewrite the
    // effective path via `strip_prefix` or short-circuit, e.g. CORS preflight).
    let mut response = match gateway.router.route(&host, &path, method.as_str()) {
        None => not_found(),
        Some(route) => {
            let mut view = pulsate_pipeline::RequestView {
                method: method.as_str().to_string(),
                path: path.clone(),
                // WAF inspects a percent-decoded form so encoded payloads
                // (`union%20select`) cannot evade signatures.
                path_and_query: percent_decode(&full_target),
                origin: origin.clone(),
                client_ip: Some(peer.ip()),
            };
            if let Some(short) = pulsate_pipeline::ingress(&route.middleware, &mut view) {
                short // security/CORS short-circuit; never cached
            } else if let Some(hit) = cache_lookup(route, &method, &host, &path, &parts.headers) {
                hit
            } else {
                let eff_path = view.path;
                let eff_pq = match &query {
                    Some(q) => format!("{eff_path}?{q}"),
                    None => eff_path.clone(),
                };
                let mut resp = match &route.handler {
                    Handler::Proxy { upstream, target } => {
                        proxy(
                            gateway,
                            upstream.as_deref(),
                            target.as_deref(),
                            &method,
                            &eff_pq,
                            &parts.headers,
                            body.clone(),
                            peer,
                            &host,
                        )
                        .await
                    }
                    other => pulsate_http::handlers::execute(other, &eff_path).await,
                };
                pulsate_pipeline::egress(&route.middleware, origin.as_deref(), &mut resp);
                cache_store(route, &method, &host, &path, &parts.headers, &mut resp);
                resp
            }
        }
    };

    // [Finalize]: stamp the request ID, advertise HTTP/3, record metrics, log.
    if let Ok(v) = http::HeaderValue::from_str(&rid) {
        response
            .headers_mut()
            .insert(http::HeaderName::from_static("x-request-id"), v);
    }
    if let Some(alt) = &gateway.alt_svc {
        if let Ok(v) = http::HeaderValue::from_str(alt) {
            response
                .headers_mut()
                .insert(http::HeaderName::from_static("alt-svc"), v);
        }
    }
    let status = response.status().as_u16();
    let bytes = response.body().len_hint().unwrap_or(0) as u64;
    let dur = started.elapsed();
    gateway
        .telemetry
        .record_request(method.as_str(), status, dur.as_secs_f64());
    let line = AccessLog {
        ts_ms: pulsate_observe::now_ms(),
        method: method.as_str(),
        host: &host,
        path: &path,
        status,
        dur_ms: dur.as_secs_f64() * 1000.0,
        request_id: &rid,
        bytes,
    }
    .to_json();
    println!("{line}");

    response
}

/// Forward a request to the upstream named by `@name` or a direct `target` URL.
#[allow(clippy::too_many_arguments)]
async fn proxy(
    gateway: &Gateway,
    upstream: Option<&str>,
    target: Option<&str>,
    method: &http::Method,
    path_and_query: &str,
    headers: &http::HeaderMap,
    body: Bytes,
    peer: SocketAddr,
    host: &str,
) -> Response {
    let pool = match (upstream, target) {
        (Some(name), _) => gateway.upstreams.get(name),
        // A direct `proxy(http://host:port)` becomes a one-target ephemeral pool.
        (None, Some(url)) => Some(Arc::new(Upstream::new(
            "_direct",
            [(url.to_string(), 1)],
            Policy::RoundRobin,
            pulsate_proxy::RetryPolicy::default(),
            pulsate_proxy::BreakerPolicy::default(),
        ))),
        (None, None) => None,
    };

    let Some(up) = pool else {
        return Response::new(http::StatusCode::BAD_GATEWAY).with_body("upstream not found");
    };
    pulsate_proxy::forward(
        &gateway.client,
        &up,
        method,
        path_and_query,
        headers,
        body,
        Some(peer.ip()),
        host,
    )
    .await
}

/// Try to serve `route`'s cache for this request, returning a hit response with
/// `Age` and `X-Cache: HIT`/`STALE` headers, or `None` to fall through.
fn cache_lookup(
    route: &Route,
    method: &http::Method,
    host: &str,
    path: &str,
    req_headers: &http::HeaderMap,
) -> Option<Response> {
    let cache = route.cache.as_ref()?;
    if !cache.request_allows_cache(method.as_str(), req_headers) {
        return None;
    }
    let key = cache.key(method.as_str(), host, path, req_headers);
    let hit = cache.lookup(&key)?;

    let mut resp = Response::new(hit.status);
    for (name, value) in &hit.headers {
        if let (Ok(n), Ok(v)) = (
            http::HeaderName::try_from(name.as_str()),
            http::HeaderValue::from_str(value),
        ) {
            resp.headers_mut().insert(n, v);
        }
    }
    set_header(&mut resp, "age", &hit.age_secs.to_string());
    let state = if hit.freshness == pulsate_cache::Freshness::Stale {
        "STALE"
    } else {
        "HIT"
    };
    set_header(&mut resp, "x-cache", state);
    Some(resp.with_body(hit.body))
}

/// Store a fresh response into `route`'s cache if it is cacheable, stamping
/// `X-Cache: MISS` (or `BYPASS` when not stored).
fn cache_store(
    route: &Route,
    method: &http::Method,
    host: &str,
    path: &str,
    req_headers: &http::HeaderMap,
    resp: &mut Response,
) {
    let Some(cache) = route.cache.as_ref() else {
        return;
    };
    if !cache.request_allows_cache(method.as_str(), req_headers) {
        set_header(resp, "x-cache", "BYPASS");
        return;
    }
    let body = match resp.body() {
        Body::Bytes(b) => b.clone(),
        _ => Bytes::new(),
    };
    let key = cache.key(method.as_str(), host, path, req_headers);
    let stored = cache.maybe_store(&key, resp.status(), resp.headers(), &body);
    set_header(resp, "x-cache", if stored { "MISS" } else { "BYPASS" });
}

/// Percent-decode a target string (best-effort, lossy on invalid UTF-8). Used
/// only to give the WAF a normalized view; the raw target is forwarded as-is.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
                i += 3;
                continue;
            }
        }
        // `+` is a space in query strings.
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn set_header(resp: &mut Response, name: &'static str, value: &str) {
    if let Ok(v) = http::HeaderValue::from_str(value) {
        resp.headers_mut()
            .insert(http::HeaderName::from_static(name), v);
    }
}

fn not_found() -> Response {
    let mut r = Response::new(http::StatusCode::NOT_FOUND);
    r.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r.with_body("no route matched")
}

/// Extract the request host from the `Host` header or the URI authority.
fn request_host(parts: &http::request::Parts) -> String {
    parts
        .headers
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string)
        .or_else(|| parts.uri.authority().map(ToString::to_string))
        .unwrap_or_default()
}
