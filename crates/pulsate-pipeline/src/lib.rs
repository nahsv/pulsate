//! `pulsate-pipeline` — the middleware engine and built-in middleware.
//!
//! Implements the [Ingress] and [Egress] halves of the request lifecycle
//! (`docs/07-middleware.md`): Ingress middleware run in declared order and may
//! short-circuit (return a response without reaching the handler); Egress
//! middleware run in reverse so wrapping concerns nest correctly.
//!
//! The runtime [`Mw`] enum lives here (a data-plane crate) so the router can
//! carry a compiled pipeline and the HTTP layer can execute it, without either
//! depending on the control-plane config crate.
#![forbid(unsafe_code)]

use std::net::IpAddr;
use std::sync::Arc;

use http::{HeaderName, HeaderValue, StatusCode};
use pulsate_core::Response;
use pulsate_waf::{IpAcl, RateLimiter, WafEngine};

/// A compiled built-in middleware.
#[derive(Clone)]
pub enum Mw {
    /// Strip a leading path prefix before the handler runs (`strip_prefix("/api")`).
    StripPrefix(String),
    /// Set and/or remove response headers (`headers(set={...}, remove=[...])`).
    Headers {
        /// Headers to set (name, value).
        set: Vec<(String, String)>,
        /// Header names to remove.
        remove: Vec<String>,
    },
    /// CORS handling (`cors(origins=[...], methods=[...], credentials=...)`).
    Cors(Cors),
    /// Rate limiting (`rate_limit(N/window, key=ip)`).
    RateLimit {
        /// The shared limiter.
        limiter: Arc<RateLimiter>,
        /// Key dimension (`ip`).
        key: String,
    },
    /// WAF signature inspection (`waf(@name)`).
    Waf(Arc<WafEngine>),
    /// IP allow/deny (`waf(@name)` with an `ip { ... }` block).
    Ip(Arc<IpAcl>),
}

impl std::fmt::Debug for Mw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mw::StripPrefix(p) => write!(f, "StripPrefix({p:?})"),
            Mw::Headers { .. } => f.write_str("Headers"),
            Mw::Cors(_) => f.write_str("Cors"),
            Mw::RateLimit { key, .. } => write!(f, "RateLimit(key={key})"),
            Mw::Waf(_) => f.write_str("Waf"),
            Mw::Ip(_) => f.write_str("Ip"),
        }
    }
}

/// CORS configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Cors {
    /// Allowed origins (`*` allows any).
    pub origins: Vec<String>,
    /// Allowed methods.
    pub methods: Vec<String>,
    /// Whether to allow credentials.
    pub credentials: bool,
}

/// The mutable request view Ingress middleware operate on.
#[derive(Debug)]
pub struct RequestView {
    /// The request method.
    pub method: String,
    /// The effective path (middleware like `strip_prefix` rewrite this).
    pub path: String,
    /// The full path + query string (what the WAF inspects).
    pub path_and_query: String,
    /// The request `Origin` header, if present (for CORS).
    pub origin: Option<String>,
    /// The client IP (for rate limiting and IP ACLs).
    pub client_ip: Option<IpAddr>,
}

/// Run the Ingress phase. Returns `Some(response)` if a middleware short-circuits
/// (e.g. a CORS preflight), in which case the handler is skipped and Egress runs
/// on the returned response.
#[must_use]
pub fn ingress(pipeline: &[Mw], req: &mut RequestView) -> Option<Response> {
    for mw in pipeline {
        match mw {
            Mw::StripPrefix(prefix) => {
                if let Some(rest) = req.path.strip_prefix(prefix.as_str()) {
                    req.path = normalize_path(rest);
                }
            }
            Mw::Cors(cors) => {
                if req.method.eq_ignore_ascii_case("OPTIONS") && req.origin.is_some() {
                    return Some(preflight(cors, req.origin.as_deref()));
                }
            }
            Mw::Ip(acl) => {
                if let Some(ip) = req.client_ip {
                    if let Some(block) = acl.check(ip) {
                        return Some(blocked(block.status));
                    }
                }
            }
            Mw::Waf(engine) => {
                if let Some(block) = engine.inspect(&req.path_and_query) {
                    return Some(blocked(block.status));
                }
            }
            Mw::RateLimit { limiter, key } => {
                let bucket = rate_key(key, req);
                let outcome = limiter.check(&bucket);
                if !outcome.allowed {
                    return Some(rate_limited(&outcome));
                }
            }
            Mw::Headers { .. } => { /* response-phase only */ }
        }
    }
    None
}

/// Build the rate-limit bucket key for a request.
fn rate_key(key: &str, req: &RequestView) -> String {
    match key {
        // Canonicalize so an IPv4-mapped IPv6 client shares one bucket with its
        // plain-v4 form and cannot multiply its budget (M7).
        "ip" => req
            .client_ip
            .map_or_else(|| "unknown".to_string(), |ip| ip.to_canonical().to_string()),
        other => format!("{other}:{}", req.path),
    }
}

/// A bare blocked response (security blocks never leak detail to the client).
fn blocked(status: u16) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::FORBIDDEN);
    let mut resp = Response::new(code);
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp.with_body("forbidden")
}

/// A 429 response carrying the standard `RateLimit-*` and `Retry-After` headers.
fn rate_limited(outcome: &pulsate_waf::RateOutcome) -> Response {
    let mut resp = Response::new(StatusCode::TOO_MANY_REQUESTS);
    let headers = resp.headers_mut();
    insert_num(headers, "ratelimit-limit", outcome.limit);
    insert_num(headers, "ratelimit-remaining", outcome.remaining);
    insert_num(headers, "ratelimit-reset", outcome.reset_secs);
    insert_num(headers, "retry-after", outcome.reset_secs);
    resp.with_body("rate limit exceeded")
}

fn insert_num(headers: &mut http::HeaderMap, name: &'static str, value: u64) {
    if let Ok(v) = HeaderValue::from_str(&value.to_string()) {
        headers.insert(HeaderName::from_static(name), v);
    }
}

/// Run the Egress phase (reverse order), mutating the response.
pub fn egress(pipeline: &[Mw], origin: Option<&str>, resp: &mut Response) {
    for mw in pipeline.iter().rev() {
        match mw {
            Mw::Headers { set, remove } => {
                for name in remove {
                    if let Ok(h) = HeaderName::try_from(name.as_str()) {
                        resp.headers_mut().remove(&h);
                    }
                }
                for (name, value) in set {
                    if let (Ok(h), Ok(v)) = (
                        HeaderName::try_from(name.as_str()),
                        HeaderValue::from_str(value),
                    ) {
                        resp.headers_mut().insert(h, v);
                    }
                }
            }
            Mw::Cors(cors) => apply_cors(cors, origin, resp),
            // Ingress-only middleware do nothing on the response phase.
            Mw::StripPrefix(_) | Mw::RateLimit { .. } | Mw::Waf(_) | Mw::Ip(_) => {}
        }
    }
}

/// Normalize a stripped path back to an absolute path.
fn normalize_path(rest: &str) -> String {
    if rest.is_empty() {
        "/".to_string()
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

fn origin_allowed(cors: &Cors, origin: Option<&str>) -> Option<String> {
    let origin = origin?;
    if cors.origins.iter().any(|o| o == "*") {
        Some("*".to_string())
    } else if cors.origins.iter().any(|o| o == origin) {
        Some(origin.to_string())
    } else {
        None
    }
}

fn apply_cors(cors: &Cors, origin: Option<&str>, resp: &mut Response) {
    if let Some(allow) = origin_allowed(cors, origin) {
        let headers = resp.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&allow) {
            headers.insert(HeaderName::from_static("access-control-allow-origin"), v);
        }
        if cors.credentials {
            headers.insert(
                HeaderName::from_static("access-control-allow-credentials"),
                HeaderValue::from_static("true"),
            );
        }
    }
}

fn preflight(cors: &Cors, origin: Option<&str>) -> Response {
    let mut resp = Response::new(StatusCode::NO_CONTENT);
    apply_cors(cors, origin, &mut resp);
    if !cors.methods.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&cors.methods.join(", ")) {
            resp.headers_mut()
                .insert(HeaderName::from_static("access-control-allow-methods"), v);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(method: &str, path: &str, origin: Option<&str>) -> RequestView {
        RequestView {
            method: method.to_string(),
            path: path.to_string(),
            path_and_query: path.to_string(),
            origin: origin.map(ToString::to_string),
            client_ip: None,
        }
    }

    #[test]
    fn strip_prefix_rewrites_path() {
        let pipe = vec![Mw::StripPrefix("/api".to_string())];
        let mut req = view("GET", "/api/orders", None);
        assert!(ingress(&pipe, &mut req).is_none());
        assert_eq!(req.path, "/orders");

        // Stripping the whole prefix yields root.
        let mut req = view("GET", "/api", None);
        let _ = ingress(&pipe, &mut req);
        assert_eq!(req.path, "/");
    }

    #[test]
    fn headers_set_and_remove_on_egress() {
        let pipe = vec![Mw::Headers {
            set: vec![("x-frame-options".into(), "DENY".into())],
            remove: vec!["server".into()],
        }];
        let mut resp = Response::new(StatusCode::OK);
        resp.headers_mut()
            .insert("server", HeaderValue::from_static("secret"));
        egress(&pipe, None, &mut resp);
        assert_eq!(resp.headers().get("x-frame-options").unwrap(), "DENY");
        assert!(resp.headers().get("server").is_none());
    }

    #[test]
    fn cors_preflight_short_circuits() {
        let pipe = vec![Mw::Cors(Cors {
            origins: vec!["https://app.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            credentials: true,
        })];
        let mut req = view("OPTIONS", "/api", Some("https://app.example.com"));
        let resp = ingress(&pipe, &mut req).expect("preflight short-circuits");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            resp.headers().get("access-control-allow-origin").unwrap(),
            "https://app.example.com"
        );
    }

    #[test]
    fn cors_disallowed_origin_gets_no_header() {
        let pipe = vec![Mw::Cors(Cors {
            origins: vec!["https://allowed.com".into()],
            methods: vec![],
            credentials: false,
        })];
        let mut resp = Response::new(StatusCode::OK);
        egress(&pipe, Some("https://evil.com"), &mut resp);
        assert!(resp.headers().get("access-control-allow-origin").is_none());
    }
}
