//! The admin HTTP server: accept loop, routing, auth, and endpoint handlers.

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use pulsate_core::{Code, Lifecycle};
use tokio::sync::watch;

use crate::json::{field, num_field, object};
use crate::{AdminApi, Scope, Scopes};

/// Run the admin API server until the lifecycle leaves `Running`.
pub async fn serve_admin(
    listener: tokio::net::TcpListener,
    api: Arc<AdminApi>,
    mut lifecycle: watch::Receiver<Lifecycle>,
) {
    loop {
        tokio::select! {
            changed = lifecycle.changed() => {
                if changed.is_err() || *lifecycle.borrow() != Lifecycle::Running {
                    break;
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let api = Arc::clone(&api);
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(move |req: Request<Incoming>| {
                        let api = Arc::clone(&api);
                        async move { Ok::<_, Infallible>(dispatch(req, &api).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await;
                });
            }
        }
    }
}

async fn dispatch(req: Request<Incoming>, api: &AdminApi) -> Response<Full<Bytes>> {
    let (parts, body) = req.into_parts();
    let method = parts.method;
    let path = parts.uri.path().to_string();

    // The dashboard and health check are unauthenticated (loopback-only surface).
    if method == Method::GET && (path == "/" || !path.starts_with("/v1")) {
        let (ct, html) = pulsate_dashboard::asset(&path);
        return raw(StatusCode::OK, ct, html.to_string());
    }
    if method == Method::GET && path == "/v1/health" {
        return json(StatusCode::OK, object(&[field("status", "ok")]));
    }

    match (&method, path.as_str()) {
        (&Method::GET, "/v1/info") => guard(api, &parts.headers, Scope::Read, |_| info(api)),
        (&Method::GET, "/v1/metrics") => guard(api, &parts.headers, Scope::Read, |_| {
            raw(
                StatusCode::OK,
                "text/plain; version=0.0.4; charset=utf-8",
                api.gateway.telemetry.render(),
            )
        }),
        (&Method::GET, "/v1/upstreams") => {
            guard(api, &parts.headers, Scope::Read, |_| upstreams(api))
        }
        (&Method::GET, "/v1/audit") => guard(api, &parts.headers, Scope::Admin, |_| audit(api)),
        (&Method::POST, "/v1/config/validate") => {
            match authorize(api, &parts.headers, Scope::Write) {
                Err(resp) => resp,
                Ok(_) => validate_config(&collect(body).await),
            }
        }
        (&Method::POST, "/v1/config/reload") => {
            match authorize(api, &parts.headers, Scope::Write) {
                Err(resp) => resp,
                Ok(_) => reload_config(api, &collect(body).await),
            }
        }
        _ => problem(
            Code::PRX_NO_ROUTE,
            StatusCode::NOT_FOUND,
            "no such admin endpoint",
        ),
    }
}

/// Run `f` if the request carries a token with `scope`, else return the auth error.
fn guard(
    api: &AdminApi,
    headers: &HeaderMap,
    scope: Scope,
    f: impl FnOnce(Scopes) -> Response<Full<Bytes>>,
) -> Response<Full<Bytes>> {
    match authorize(api, headers, scope) {
        Ok(scopes) => f(scopes),
        Err(resp) => resp,
    }
}

/// Check the bearer token and required scope.
#[allow(clippy::result_large_err)] // the error is a full HTTP response by design
fn authorize(
    api: &AdminApi,
    headers: &HeaderMap,
    scope: Scope,
) -> Result<Scopes, Response<Full<Bytes>>> {
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);

    let Some(token) = token else {
        return Err(problem(
            Code::ADM_UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            "missing bearer token",
        ));
    };
    let Some(scopes) = api.scopes_for(token) else {
        return Err(problem(
            Code::ADM_UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            "unknown token",
        ));
    };
    if !scopes.satisfies(scope) {
        return Err(problem(
            Code::ADM_FORBIDDEN,
            StatusCode::FORBIDDEN,
            "token lacks the required scope",
        ));
    }
    Ok(scopes)
}

fn info(api: &AdminApi) -> Response<Full<Bytes>> {
    let gen = api.store.generation();
    let body = object(&[
        field("version", env!("CARGO_PKG_VERSION")),
        num_field("generation", gen),
        num_field("sites", api.gateway.router.site_count() as u64),
        num_field("upstreams", api.gateway.upstreams.len() as u64),
    ]);
    json(StatusCode::OK, body)
}

fn upstreams(api: &AdminApi) -> Response<Full<Bytes>> {
    let items: Vec<String> = api
        .gateway
        .upstreams
        .summary()
        .into_iter()
        .map(|(name, targets)| {
            object(&[field("name", &name), num_field("targets", targets as u64)])
        })
        .collect();
    json(
        StatusCode::OK,
        format!("{{\"upstreams\":[{}]}}", items.join(",")),
    )
}

fn audit(api: &AdminApi) -> Response<Full<Bytes>> {
    let entries: Vec<String> = api
        .audit
        .entries()
        .into_iter()
        .map(|e| {
            object(&[
                num_field("seq", e.seq),
                field("event", &e.event),
                field("hash", &format!("{:016x}", e.hash)),
            ])
        })
        .collect();
    json(
        StatusCode::OK,
        format!(
            "{{\"verified\":{},\"entries\":[{}]}}",
            api.audit.verify(),
            entries.join(",")
        ),
    )
}

fn validate_config(text: &str) -> Response<Full<Bytes>> {
    match pulsate_config::compile("admin", text, 0) {
        Ok(compiled) => json(
            StatusCode::OK,
            format!(
                "{{\"valid\":true,\"problems\":[{}]}}",
                problems_json(&compiled.warnings)
            ),
        ),
        Err(diags) => json(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "{{\"valid\":false,\"problems\":[{}]}}",
                problems_json(&diags)
            ),
        ),
    }
}

fn reload_config(api: &AdminApi, text: &str) -> Response<Full<Bytes>> {
    match api.store.reload("admin", text) {
        Ok(generation) => json(
            StatusCode::OK,
            object(&[
                field("ok", "true"),
                num_field("generation", generation.snapshot.generation()),
            ]),
        ),
        Err(diags) => json(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{{\"ok\":false,\"problems\":[{}]}}", problems_json(&diags)),
        ),
    }
}

fn problems_json(diags: &[pulsate_config::Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| {
            let span = d.span();
            object(&[
                field("code", &d.code().to_string()),
                num_field("line", u64::from(span.line)),
                num_field("col", u64::from(span.col)),
                field("message", d.message()),
            ])
        })
        .collect::<Vec<_>>()
        .join(",")
}

async fn collect(body: Incoming) -> String {
    let bytes = body
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn json(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    raw(status, "application/json", body)
}

fn problem(code: Code, status: StatusCode, detail: &str) -> Response<Full<Bytes>> {
    let body = object(&[
        field("type", &code.docs_url()),
        field("title", code.title()),
        num_field("status", u64::from(status.as_u16())),
        field("code", &code.to_string()),
        field("detail", detail),
    ]);
    raw(status, "application/problem+json", body)
}

fn raw(status: StatusCode, content_type: &str, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, content_type)
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"{}"))))
}
