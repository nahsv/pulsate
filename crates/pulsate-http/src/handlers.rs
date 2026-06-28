//! Terminal handler execution.
//!
//! Turns a matched [`pulsate_router::Handler`] into a [`pulsate_core::Response`].
//! Executes `respond`, `redirect`, and `files` (static serving with a
//! path-traversal guard and `try_files` fallbacks). `proxy` and unknown
//! handlers return `501 Not Implemented`.

use std::path::{Component, Path, PathBuf};

use bytes::Bytes;
use http::{header, HeaderValue, StatusCode};
use pulsate_core::Response;
use pulsate_router::Handler;

/// Execute a handler for a request to `path`, producing a response.
pub async fn execute(handler: &Handler, path: &str) -> Response {
    match handler {
        Handler::Respond { status, body } => respond(*status, body),
        Handler::Redirect { to, status } => redirect(*status, to),
        Handler::Files { root, try_files } => serve_files(root, try_files, path).await,
        Handler::Proxy { .. } | Handler::Other(_) => not_implemented(&handler.name_owned()),
    }
}

fn status_or(code: u16, fallback: StatusCode) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(fallback)
}

fn respond(status: u16, body: &str) -> Response {
    let mut resp = Response::new(status_or(status, StatusCode::OK));
    if !body.is_empty() {
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
    }
    resp.with_body(body.to_string())
}

fn redirect(status: u16, to: &str) -> Response {
    let mut resp = Response::new(status_or(status, StatusCode::PERMANENT_REDIRECT));
    if let Ok(value) = HeaderValue::from_str(to) {
        resp.headers_mut().insert(header::LOCATION, value);
    }
    resp
}

fn not_implemented(name: &str) -> Response {
    respond(
        501,
        &format!("handler `{name}` is not implemented in this build"),
    )
}

async fn serve_files(root: &str, try_files: &[String], req_path: &str) -> Response {
    let candidates: Vec<String> = if try_files.is_empty() {
        vec![req_path.to_string()]
    } else {
        try_files
            .iter()
            .map(|t| t.replace("{path}", req_path))
            .collect()
    };

    for candidate in candidates {
        if let Some(resp) = try_serve_one(root, &candidate).await {
            return resp;
        }
    }
    respond(404, "not found")
}

/// Try to serve one candidate path under `root`. Returns `None` if it does not
/// resolve to a readable file, so the caller can try the next fallback.
async fn try_serve_one(root: &str, rel: &str) -> Option<Response> {
    // A traversal attempt yields `None` here, refusing the candidate.
    let safe = sanitized_join(root, rel)?;

    // A directory request serves its index.html.
    let target = if tokio::fs::metadata(&safe).await.ok()?.is_dir() {
        safe.join("index.html")
    } else {
        safe
    };

    let bytes = tokio::fs::read(&target).await.ok()?;
    let mut resp = Response::new(StatusCode::OK);
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&target)),
    );
    Some(resp.with_body(Bytes::from(bytes)))
}

/// Join `rel` onto `root`, rejecting any `..` traversal. `None` means the path
/// escaped the root and must not be served.
fn sanitized_join(root: &str, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    let mut out = PathBuf::from(root);
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            // Anything that could climb out of root is rejected outright.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

trait HandlerName {
    fn name_owned(&self) -> String;
}
impl HandlerName for Handler {
    fn name_owned(&self) -> String {
        match self {
            Handler::Files { .. } => "files",
            Handler::Respond { .. } => "respond",
            Handler::Redirect { .. } => "redirect",
            Handler::Proxy { .. } => "proxy",
            Handler::Other(n) => n,
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn respond_sets_status_and_body() {
        let h = Handler::Respond {
            status: 418,
            body: "teapot".into(),
        };
        let resp = execute(&h, "/").await;
        assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(resp.body().len_hint(), Some(6));
    }

    #[tokio::test]
    async fn redirect_sets_location() {
        let h = Handler::Redirect {
            to: "https://example.com/".into(),
            status: 308,
        };
        let resp = execute(&h, "/old").await;
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            resp.headers().get(header::LOCATION).unwrap(),
            "https://example.com/"
        );
    }

    #[tokio::test]
    async fn files_serves_and_blocks_traversal() {
        let dir = std::env::temp_dir().join(format!("pulsate-files-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("index.html"), b"<h1>hi</h1>")
            .await
            .unwrap();

        let root = dir.to_string_lossy().to_string();
        let h = Handler::Files {
            root: root.clone(),
            try_files: Vec::new(),
        };

        // Directory request serves index.html.
        let resp = execute(&h, "/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );

        // Traversal is refused (served as 404, never escaping root).
        let resp = execute(&h, "/../../etc/passwd").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[test]
    fn sanitized_join_rejects_parent_dirs() {
        assert!(sanitized_join("/srv", "/../etc").is_none());
        assert!(sanitized_join("/srv", "a/../../b").is_none());
        assert_eq!(
            sanitized_join("/srv", "/css/app.css"),
            Some(PathBuf::from("/srv/css/app.css"))
        );
    }
}
