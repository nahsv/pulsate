//! `pulsate-dashboard` — the embedded operator dashboard.
//!
//! A single-file vanilla-JS dashboard, compiled into the binary as a string
//! constant and served by the admin server (`docs/11-dashboard.md`). It polls the
//! Admin API for overview, upstreams, audit, and metrics.
#![forbid(unsafe_code)]

/// The dashboard index page.
pub const INDEX_HTML: &str = include_str!("index.html");

/// Look up an embedded asset by request path. Any unknown path falls back to the
/// SPA index so client-side routing works.
#[must_use]
pub fn asset(_path: &str) -> (&'static str, &'static str) {
    // (content_type, body) — one asset; every path serves the index.
    ("text/html; charset=utf-8", INDEX_HTML)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_embedded() {
        assert!(INDEX_HTML.contains("Pulsate"));
        assert_eq!(asset("/").0, "text/html; charset=utf-8");
        assert!(asset("/anything").1.contains("<html"));
    }
}
