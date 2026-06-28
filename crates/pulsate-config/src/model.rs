//! The typed configuration model.
//!
//! This is what the AST lowers into (`docs/02-architecture.md#configuration-loading`):
//! a validated, domain-meaningful [`Config`] that the snapshot builder compiles
//! into a [`pulsate_core::ConfigSnapshot`]. It models the structural backbone the
//! routing, proxy, and TLS subsystems hang off — enough to validate references,
//! handler counts, and host collisions, and to hash deterministically.

use pulsate_flow::Span;

/// A fully-parsed, validated configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The `flow_version` pin, if the file declared one.
    pub flow_version: Option<String>,
    /// Named upstream pools (with their targets and policies).
    pub upstreams: Vec<Upstream>,
    /// Named caches.
    pub caches: Vec<CacheDef>,
    /// Named WAF rulesets.
    pub wafs: Vec<WafDef>,
    /// Named user sets (for `basic_auth`).
    pub user_sets: Vec<Named>,
    /// Sites and their routes.
    pub sites: Vec<Site>,
}

impl Config {
    /// Every defined `@name`, across all reference-able kinds, with its span.
    #[must_use]
    pub fn defined_names(&self) -> Vec<NameRef<'_>> {
        let ups = self.upstreams.iter().map(|u| (u.name.as_str(), u.span));
        let caches = self.caches.iter().map(|c| (c.name.as_str(), c.span));
        let wafs = self.wafs.iter().map(|w| (w.name.as_str(), w.span));
        let users = self.user_sets.iter().map(|n| (n.name.as_str(), n.span));
        ups.chain(caches)
            .chain(wafs)
            .chain(users)
            .map(|(name, span)| NameRef { name, span })
            .collect()
    }

    /// Look up a cache definition by name.
    #[must_use]
    pub fn cache(&self, name: &str) -> Option<&CacheDef> {
        self.caches.iter().find(|c| c.name == name)
    }

    /// Look up a WAF definition by name.
    #[must_use]
    pub fn waf(&self, name: &str) -> Option<&WafDef> {
        self.wafs.iter().find(|w| w.name == name)
    }
}

/// A named cache definition (`cache <name> { ... }`).
#[derive(Debug, Clone, PartialEq)]
pub struct CacheDef {
    /// The cache name.
    pub name: String,
    /// The defining span.
    pub span: Span,
    /// Default freshness TTL in seconds.
    pub default_ttl_secs: u64,
    /// Cacheable methods.
    pub methods: Vec<String>,
    /// `Vary` request-header dimensions for the key.
    pub vary: Vec<String>,
    /// Stale-while-revalidate window in seconds.
    pub swr_secs: u64,
}

/// A named WAF definition (`waf <name> { ... }`).
#[derive(Debug, Clone, PartialEq)]
pub struct WafDef {
    /// The WAF name.
    pub name: String,
    /// The defining span.
    pub span: Span,
    /// Mode keyword (`block` or `detect`).
    pub mode: String,
    /// IP CIDRs to deny.
    pub ip_deny: Vec<String>,
    /// IP CIDRs to allow (override a deny).
    pub ip_allow: Vec<String>,
}

/// A compiled-but-unresolved middleware spec. `@ref`-bearing specs (`Waf`,
/// `Cache`) are resolved against the config when the router is built.
#[derive(Debug, Clone, PartialEq)]
pub enum MwSpec {
    /// `strip_prefix("/api")`.
    StripPrefix(String),
    /// `cors(origins=[...], methods=[...], credentials=...)`.
    Cors {
        /// Allowed origins.
        origins: Vec<String>,
        /// Allowed methods.
        methods: Vec<String>,
        /// Allow credentials.
        credentials: bool,
    },
    /// `rate_limit(N/window, key=ip)`.
    RateLimit {
        /// Request count per window.
        count: u64,
        /// Window length in seconds.
        per_secs: u64,
        /// Key dimension (`ip`).
        key: String,
    },
    /// `waf(@name)` — resolved to a WAF engine at router-build time.
    Waf(String),
    /// `cache(@name)` — resolved to a cache layer at router-build time.
    Cache(String),
}

/// A borrowed reference to a defined name and its span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NameRef<'a> {
    /// The defined name.
    pub name: &'a str,
    /// Where it was defined.
    pub span: Span,
}

/// A named, reference-able definition (`cache www { ... }` → `@www`).
#[derive(Debug, Clone, PartialEq)]
pub struct Named {
    /// The name used in `@references`.
    pub name: String,
    /// The defining span (for duplicate/collision diagnostics).
    pub span: Span,
}

/// A named upstream pool: backend targets, a balancing policy, and resilience
/// policy.
#[derive(Debug, Clone, PartialEq)]
pub struct Upstream {
    /// The name used in `@references`.
    pub name: String,
    /// The defining span.
    pub span: Span,
    /// Backend targets.
    pub targets: Vec<Target>,
    /// Load-balancing policy keyword (`round_robin`, `least_conn`, …).
    pub policy: String,
    /// Retry policy, if configured.
    pub retry: Option<Retry>,
    /// Circuit-breaker / passive-ejection policy, if configured.
    pub breaker: Option<Breaker>,
}

/// One backend target of an upstream.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The target URL (`http://10.0.0.1:8080`).
    pub url: String,
    /// Relative weight for weighted policies (default 1).
    pub weight: u32,
}

/// Retry policy for an upstream.
#[derive(Debug, Clone, PartialEq)]
pub struct Retry {
    /// Maximum additional attempts after the first.
    pub attempts: u32,
    /// Response statuses that trigger a retry.
    pub retry_on_status: Vec<u16>,
    /// Whether a connect error triggers a retry.
    pub on_connect_error: bool,
}

/// Passive circuit-breaker / ejection policy for an upstream.
#[derive(Debug, Clone, PartialEq)]
pub struct Breaker {
    /// Consecutive failures before a target is ejected.
    pub consecutive_failures: u32,
    /// How long (seconds) an ejected target stays out.
    pub open_for_secs: u64,
}

/// How a site terminates TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsMode {
    /// Automatic HTTPS via ACME (the secure-by-default mode).
    Auto,
    /// Explicitly disabled (`tls off`).
    Off,
    /// Manual or explicit TLS configured via a `tls { ... }` block.
    Manual,
}

/// A site: one or more hosts and the routes served for them.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    /// The host patterns this site serves.
    pub hosts: Vec<Host>,
    /// The TLS mode (defaults to [`TlsMode::Auto`] — secure by default).
    pub tls: TlsMode,
    /// The site's routes.
    pub routes: Vec<RouteDef>,
    /// The site block's span.
    pub span: Span,
}

/// A host pattern with its span.
#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    /// The host string (`example.com`, `*.preview.example.com`, `:default`).
    pub pattern: String,
    /// Its span.
    pub span: Span,
}

/// How a route's pattern is matched (mirrors `pulsate_flow::ast::MatchKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// Prefix / glob match (`/api/*`).
    Prefix,
    /// Exact path (`= /healthz`).
    Exact,
    /// Regex (`~ ^/u/...$`).
    Regex,
}

/// A lowered route: matcher, middleware names, and the terminal handler.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteDef {
    /// How `pattern` is matched.
    pub kind: MatchKind,
    /// The raw matcher pattern (e.g. `/api/*`).
    pub pattern: String,
    /// An optional method refinement from `[method=GET]`.
    pub method: Option<String>,
    /// The names of the middleware steps, in pipeline order (for diagnostics).
    pub middleware: Vec<String>,
    /// The compiled-but-unresolved middleware specs (resolved at router build).
    pub mw_specs: Vec<MwSpec>,
    /// The terminal handler, if the route has one.
    pub handler: Option<Handler>,
    /// Every `@ref` the route mentions, with spans, for integrity checking.
    pub refs: Vec<RefUse>,
    /// The route's span.
    pub span: Span,
}

/// A terminal route handler, with the arguments needed to execute it.
#[derive(Debug, Clone, PartialEq)]
pub enum Handler {
    /// Serve static files from `root`, with optional `try_files` fallbacks.
    Files {
        /// Filesystem root directory.
        root: String,
        /// Fallback path templates (e.g. `["{path}", "/index.html"]`).
        try_files: Vec<String>,
    },
    /// Return an inline response.
    Respond {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },
    /// Redirect to another location.
    Redirect {
        /// Target location.
        to: String,
        /// Redirect status (default 308).
        status: u16,
    },
    /// Reverse-proxy to a pool or target.
    Proxy {
        /// A `@upstream` reference, if used.
        upstream: Option<String>,
        /// A direct target URL, if used.
        target: Option<String>,
    },
    /// A recognized handler keyword with no typed lowering.
    Other(String),
}

impl Handler {
    /// The handler's keyword name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Handler::Files { .. } => "files",
            Handler::Respond { .. } => "respond",
            Handler::Redirect { .. } => "redirect",
            Handler::Proxy { .. } => "proxy",
            Handler::Other(n) => n,
        }
    }
}

/// A use of a `@reference` somewhere in a route, retained for validation.
#[derive(Debug, Clone, PartialEq)]
pub struct RefUse {
    /// The referenced name (without the `@`).
    pub name: String,
    /// Where it was used.
    pub span: Span,
}
