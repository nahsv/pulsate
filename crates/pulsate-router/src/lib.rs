//! `pulsate-router` — the routing table and request matchers.
//!
//! Compiles a set of sites and routes into a table the data plane consults at
//! the [`Match`](pulsate_core::Stage::Match) stage. Matching is deterministic and
//! precedence-based — **exact > longer prefix > regex > catch-all** — so the
//! result never depends on declaration order (`docs/06-reverse-proxy.md`).
//!
//! The runtime [`Handler`] type lives here (a data-plane crate) so the router
//! and the HTTP serving layer share it without depending on the control-plane
//! config crate. `pulsate-config` converts its typed model into a [`Router`].
#![forbid(unsafe_code)]

/// How a route pattern is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// Prefix / glob (`/api/*`).
    Prefix,
    /// Exact path (`= /healthz`).
    Exact,
    /// Regex (`~ ^/u/...$`). Matched only as a literal; there is no regex engine.
    Regex,
}

/// A terminal handler with the data needed to execute it.
#[derive(Debug, Clone, PartialEq)]
pub enum Handler {
    /// Serve static files from `root`, trying `try_files` templates in order.
    Files {
        /// Filesystem root.
        root: String,
        /// Fallback path templates (`{path}` is substituted).
        try_files: Vec<String>,
    },
    /// Return an inline response.
    Respond {
        /// Status code.
        status: u16,
        /// Body.
        body: String,
    },
    /// Redirect to a location.
    Redirect {
        /// Target location.
        to: String,
        /// Redirect status.
        status: u16,
    },
    /// Reverse proxy.
    Proxy {
        /// A `@upstream` name, if used.
        upstream: Option<String>,
        /// A direct target URL, if used.
        target: Option<String>,
    },
    /// A handler keyword this crate does not execute.
    Other(String),
}

/// A compiled route: a matcher, an optional method refinement, the middleware
/// pipeline, an optional cache layer, and a terminal handler.
#[derive(Debug, Clone)]
pub struct Route {
    /// How `pattern` is matched.
    pub kind: MatchKind,
    /// The path/regex pattern.
    pub pattern: String,
    /// Optional method refinement (uppercased), e.g. `POST`.
    pub method: Option<String>,
    /// The compiled middleware pipeline (Ingress order).
    pub middleware: Vec<pulsate_pipeline::Mw>,
    /// An optional response cache for this route (`cache(@name)`).
    pub cache: Option<pulsate_cache::CacheLayer>,
    /// The terminal handler.
    pub handler: Handler,
}

impl Route {
    /// Whether this route matches `path`+`method`, and with what specificity
    /// score (higher binds first). Returns `None` if it does not match.
    fn score(&self, path: &str, method: &str) -> Option<u32> {
        if let Some(m) = &self.method {
            if !m.eq_ignore_ascii_case(method) {
                return None;
            }
        }
        match self.kind {
            MatchKind::Exact => (self.pattern == path).then_some(1_000_000),
            MatchKind::Prefix => {
                let prefix = prefix_of(&self.pattern);
                if prefix.is_empty() || path == prefix || path.starts_with(&with_slash(&prefix)) {
                    // Longer prefixes are more specific; offset below Exact.
                    Some(1_000 + u32::try_from(prefix.len()).unwrap_or(0))
                } else {
                    None
                }
            }
            // No regex engine: match only as a literal, lowest precedence.
            MatchKind::Regex => (self.pattern == path).then_some(10),
        }
    }
}

/// One site: its host patterns and its routes.
#[derive(Debug, Clone)]
pub struct SiteRoutes {
    /// Host patterns served (`example.com`, `*.example.com`, `:default`).
    pub hosts: Vec<String>,
    /// The site's routes.
    pub routes: Vec<Route>,
}

impl SiteRoutes {
    /// How well this site matches `host`: higher is better, `None` if no match.
    /// Exact host beats wildcard beats `:default`.
    fn host_score(&self, host: &str) -> Option<u32> {
        let mut best = None;
        for pat in &self.hosts {
            let s = if pat == host {
                Some(3)
            } else if pat == ":default" {
                Some(1)
            } else if let Some(suffix) = pat.strip_prefix("*.") {
                // `*.example.com` matches `a.example.com` but not `example.com`.
                (host.len() > suffix.len() && host.ends_with(suffix)).then_some(2)
            } else {
                None
            };
            best = best.max(s);
        }
        best
    }
}

/// The compiled routing table.
#[derive(Debug, Clone, Default)]
pub struct Router {
    sites: Vec<SiteRoutes>,
}

impl Router {
    /// Build a router from site definitions.
    #[must_use]
    pub fn new(sites: Vec<SiteRoutes>) -> Self {
        Self { sites }
    }

    /// Resolve a request to its handler, or `None` if nothing matches.
    ///
    /// First the most-specific site for `host` is chosen, then the
    /// most-specific route within it for `path`+`method`.
    #[must_use]
    pub fn route(&self, host: &str, path: &str, method: &str) -> Option<&Route> {
        let host = host_without_port(host);
        let site = self
            .sites
            .iter()
            .filter_map(|s| s.host_score(host).map(|sc| (sc, s)))
            .max_by_key(|(sc, _)| *sc)
            .map(|(_, s)| s)?;

        site.routes
            .iter()
            .filter_map(|r| r.score(path, method).map(|sc| (sc, r)))
            .max_by_key(|(sc, _)| *sc)
            .map(|(_, r)| r)
    }

    /// The number of sites in the table.
    #[must_use]
    pub fn site_count(&self) -> usize {
        self.sites.len()
    }
}

/// Strip a trailing `/*` (or `*`) glob from a prefix pattern; `/` and `/*`
/// reduce to the empty (catch-all) prefix.
fn prefix_of(pattern: &str) -> String {
    let p = pattern.strip_suffix('*').unwrap_or(pattern);
    let p = p.strip_suffix('/').unwrap_or(p);
    if p == "/" {
        String::new()
    } else {
        p.to_string()
    }
}

fn with_slash(prefix: &str) -> String {
    if prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    }
}

fn host_without_port(host: &str) -> &str {
    host.split_once(':').map_or(host, |(h, _)| h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(kind: MatchKind, pattern: &str) -> Route {
        Route {
            kind,
            pattern: pattern.to_string(),
            method: None,
            middleware: Vec::new(),
            cache: None,
            handler: Handler::Respond {
                status: 200,
                body: pattern.to_string(),
            },
        }
    }

    fn router() -> Router {
        Router::new(vec![SiteRoutes {
            hosts: vec!["app.example.com".into()],
            routes: vec![
                route(MatchKind::Prefix, "/*"),
                route(MatchKind::Prefix, "/api/*"),
                route(MatchKind::Exact, "/api/health"),
            ],
        }])
    }

    fn matched(r: &Router, path: &str) -> String {
        match &r.route("app.example.com", path, "GET").unwrap().handler {
            Handler::Respond { body, .. } => body.clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn exact_beats_prefix_beats_catch_all() {
        let r = router();
        assert_eq!(matched(&r, "/api/health"), "/api/health"); // exact
        assert_eq!(matched(&r, "/api/users"), "/api/*"); // longer prefix
        assert_eq!(matched(&r, "/home"), "/*"); // catch-all
    }

    #[test]
    fn prefix_requires_segment_boundary() {
        let r = Router::new(vec![SiteRoutes {
            hosts: vec!["h".into()],
            routes: vec![route(MatchKind::Prefix, "/api/*")],
        }]);
        assert!(r.route("h", "/api", "GET").is_some());
        assert!(r.route("h", "/api/x", "GET").is_some());
        assert!(r.route("h", "/apixyz", "GET").is_none());
    }

    #[test]
    fn method_refinement_filters() {
        let mut post = route(MatchKind::Prefix, "/api/*");
        post.method = Some("POST".into());
        let r = Router::new(vec![SiteRoutes {
            hosts: vec!["h".into()],
            routes: vec![post],
        }]);
        assert!(r.route("h", "/api/x", "POST").is_some());
        assert!(r.route("h", "/api/x", "GET").is_none());
    }

    #[test]
    fn host_exact_beats_wildcard_beats_default() {
        let r = Router::new(vec![
            SiteRoutes {
                hosts: vec![":default".into()],
                routes: vec![route(MatchKind::Prefix, "/*")],
            },
            SiteRoutes {
                hosts: vec!["*.example.com".into()],
                routes: vec![route(MatchKind::Exact, "/w")],
            },
            SiteRoutes {
                hosts: vec!["a.example.com".into()],
                routes: vec![route(MatchKind::Exact, "/e")],
            },
        ]);
        assert!(r.route("a.example.com", "/e", "GET").is_some()); // exact host
        assert!(r.route("b.example.com", "/w", "GET").is_some()); // wildcard host
        assert!(r.route("other.org", "/anything", "GET").is_some()); // :default
    }

    #[test]
    fn port_is_ignored_in_host() {
        let r = router();
        assert!(r
            .route("app.example.com:8443", "/api/health", "GET")
            .is_some());
    }
}
