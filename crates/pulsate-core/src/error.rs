//! The Pulsate error taxonomy.
//!
//! Every fallible library path returns [`Result<T, PulsateError>`]. Each error
//! carries a stable [`Code`] (`PLS-<AREA>-<NNNN>`), a [`Category`] that decides
//! how it surfaces, and a human message. Codes never change meaning — they are
//! the contract operators search, logs key on, and the [Recover] phase maps to
//! responses. See `docs/25-error-and-status-catalog.md`.

use std::fmt;

/// Convenient alias for fallible Pulsate operations.
pub type Result<T, E = PulsateError> = std::result::Result<T, E>;

/// Broad classification of an error, driving how it is surfaced to the client,
/// the logs, and the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Category {
    /// The request is bad — surfaces as a 4xx response.
    Client,
    /// A backend failed — surfaces as 502/503/504.
    Upstream,
    /// Invalid configuration — surfaces at load time / `pulsate validate` / 422.
    Config,
    /// Blocked by policy — surfaces as 401/403/429.
    Security,
    /// A Pulsate-side fault — surfaces as 500; detail never leaks to the client.
    Internal,
    /// An environment/runtime condition — startup failure, exit code, metric.
    Operational,
}

impl Category {
    /// The default HTTP status for a request-facing error of this category.
    ///
    /// Specific [`Code`]s may override this (e.g. a rate limit is `Security`
    /// but maps to 429); see [`PulsateError::http_status`].
    #[must_use]
    pub const fn default_status(self) -> u16 {
        match self {
            Category::Client | Category::Config => 400,
            Category::Upstream => 502,
            Category::Security => 403,
            Category::Internal => 500,
            Category::Operational => 503,
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Category::Client => "client",
            Category::Upstream => "upstream",
            Category::Config => "config",
            Category::Security => "security",
            Category::Internal => "internal",
            Category::Operational => "operational",
        };
        f.write_str(s)
    }
}

/// A stable Pulsate error code, e.g. `PLS-PRX-0003`.
///
/// Codes are defined in one place so docs, metrics (`pulsate_errors_total{code}`),
/// and the `problem+json` `type` URL all derive from the same registry — no
/// drift. Construct via the associated constants; the set is closed per release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Code {
    area: &'static str,
    number: u16,
    title: &'static str,
    category: Category,
    /// Request-facing HTTP status, if any. `None` for non-request errors.
    status: Option<u16>,
}

impl Code {
    /// The subsystem area (`CFG`, `HTTP`, `PRX`, `TLS`, `ACME`, `SEC`, `WAF`,
    /// `CACHE`, `PLG`, `ADM`, `CLU`, `SYS`).
    #[must_use]
    pub const fn area(&self) -> &'static str {
        self.area
    }

    /// The zero-padded stable number within the area.
    #[must_use]
    pub const fn number(&self) -> u16 {
        self.number
    }

    /// A short human title for the code.
    #[must_use]
    pub const fn title(&self) -> &'static str {
        self.title
    }

    /// The error category.
    #[must_use]
    pub const fn category(&self) -> Category {
        self.category
    }

    /// The request-facing HTTP status, or the category default if unset.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        self.status
            .unwrap_or_else(|| self.category.default_status())
    }

    /// The canonical documentation URL for this code.
    #[must_use]
    pub fn docs_url(&self) -> String {
        format!("https://squaretick.dev/pulsate/errors/{self}")
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // PLS-<AREA>-<NNNN>, number zero-padded to four digits.
        write!(f, "PLS-{}-{:04}", self.area, self.number)
    }
}

/// Defines the `PLS-*` code constants.
macro_rules! codes {
    ($( $konst:ident = ($area:literal, $num:literal, $title:literal, $cat:expr, $status:expr) ; )*) => {
        impl Code {
            $(
                #[doc = concat!("`PLS-", $area, "-", stringify!($num), "` — ", $title, ".")]
                pub const $konst: Code = Code {
                    area: $area,
                    number: $num,
                    title: $title,
                    category: $cat,
                    status: $status,
                };
            )*

            /// Every defined code, for catalog generation and the "100% of codes
            /// exercised" test gate.
            #[must_use]
            pub const fn all() -> &'static [Code] {
                &[ $( Code::$konst ),* ]
            }
        }
    };
}

use Category::{Client, Config, Internal, Operational, Security, Upstream};

codes! {
    // Config (PLS-CFG) — load-time; never affects the running snapshot.
    CFG_SYNTAX          = ("CFG", 1, "Syntax error", Config, None);
    CFG_UNKNOWN_DIRECTIVE = ("CFG", 2, "Unknown directive", Config, None);
    CFG_TYPE_MISMATCH   = ("CFG", 3, "Type/unit mismatch", Config, None);
    CFG_MISSING_FIELD   = ("CFG", 5, "Missing required field", Config, None);
    CFG_UNKNOWN_REF     = ("CFG", 7, "Unknown reference", Config, None);
    CFG_HOST_COLLISION  = ("CFG", 10, "Host+port collision", Config, None);
    CFG_MULTI_HANDLER   = ("CFG", 12, "Multiple handlers in one route", Config, None);
    CFG_ACME_UNREACHABLE = ("CFG", 15, "ACME challenge unreachable", Config, None);
    CFG_DUPLICATE       = ("CFG", 20, "Duplicate definition", Config, None);
    CFG_INVALID_CORS    = ("CFG", 21, "Invalid CORS configuration", Config, None);

    // HTTP / proxy (PLS-HTTP / PLS-PRX).
    HTTP_MALFORMED      = ("HTTP", 1, "Malformed request", Client, Some(400));
    HTTP_AMBIGUOUS_FRAMING = ("HTTP", 2, "Ambiguous framing (smuggling)", Client, Some(400));
    HTTP_HEADER_LIMITS  = ("HTTP", 3, "Header limits exceeded", Client, Some(431));
    HTTP_BODY_TOO_LARGE = ("HTTP", 4, "Body too large", Client, Some(413));
    HTTP_TIMEOUT        = ("HTTP", 5, "Request timeout", Client, Some(408));
    PRX_NO_ROUTE        = ("PRX", 1, "No route matched", Client, Some(404));
    PRX_CONNECT_TIMEOUT = ("PRX", 2, "Upstream connect timeout", Upstream, Some(504));
    PRX_NO_HEALTHY      = ("PRX", 3, "No healthy upstream target", Upstream, Some(503));
    PRX_RESPONSE_TIMEOUT = ("PRX", 4, "Upstream response timeout", Upstream, Some(504));
    PRX_BREAKER_OPEN    = ("PRX", 5, "Circuit breaker open", Upstream, Some(503));
    PRX_RETRY_EXHAUSTED = ("PRX", 6, "Retry budget exhausted", Upstream, Some(502));
    PRX_PROTOCOL_ERROR  = ("PRX", 7, "Upstream protocol error", Upstream, Some(502));

    // TLS / ACME (PLS-TLS / PLS-ACME).
    TLS_NO_CERT         = ("TLS", 1, "No certificate for SNI", Security, None);
    TLS_CLIENT_CERT     = ("TLS", 2, "Client cert required/invalid", Security, Some(403));
    TLS_PROTOCOL        = ("TLS", 3, "Protocol/cipher not permitted", Security, None);
    ACME_CHALLENGE      = ("ACME", 1, "Challenge failed", Operational, None);
    ACME_RATE_LIMITED   = ("ACME", 2, "Rate-limited by CA", Operational, None);
    ACME_RENEWAL        = ("ACME", 3, "Renewal failed", Operational, None);
    ACME_NOT_ALLOWLISTED = ("ACME", 4, "On-demand host not allow-listed", Security, Some(403));

    // Security / WAF (PLS-SEC / PLS-WAF).
    SEC_AUTH_REQUIRED   = ("SEC", 1, "Authentication required", Security, Some(401));
    SEC_TOKEN_INVALID   = ("SEC", 2, "Token invalid/expired", Security, Some(401));
    SEC_AUTHZ_DENIED    = ("SEC", 3, "Authorization denied", Security, Some(403));
    SEC_RATE_LIMIT      = ("SEC", 4, "Rate limit exceeded", Security, Some(429));
    WAF_RULE            = ("WAF", 1, "Blocked by WAF rule", Security, Some(403));
    WAF_GEO            = ("WAF", 2, "Blocked by geo/ASN policy", Security, Some(403));
    WAF_BOT             = ("WAF", 3, "Bot challenge required/failed", Security, Some(403));
    WAF_IP_DENIED       = ("WAF", 4, "IP denied", Security, Some(403));

    // Plugin / admin (PLS-PLG / PLS-ADM).
    PLG_LOAD            = ("PLG", 1, "Plugin load/validation failed", Internal, None);
    PLG_TRAPPED         = ("PLG", 2, "Plugin trapped", Internal, None);
    PLG_FUEL            = ("PLG", 3, "Plugin exceeded fuel/time", Internal, None);
    PLG_CAPABILITY      = ("PLG", 4, "Capability denied", Internal, None);
    PLG_ABI             = ("PLG", 5, "ABI version unsupported", Internal, None);
    ADM_UNAUTHORIZED    = ("ADM", 1, "Unauthorized", Security, Some(401));
    ADM_FORBIDDEN       = ("ADM", 2, "Forbidden (scope)", Security, Some(403));
    ADM_APPLY_REJECTED  = ("ADM", 3, "Config apply rejected", Config, Some(422));
    ADM_CONFLICT        = ("ADM", 4, "Conflict (optimistic concurrency)", Client, Some(409));

    // Process / runtime (PLS-SYS).
    SYS_GENERIC         = ("SYS", 1, "Generic runtime error", Internal, None);
    SYS_BIND            = ("SYS", 3, "Bind failed / port in use", Operational, None);
    SYS_STATE_STORE     = ("SYS", 6, "State store error", Operational, None);
}

/// The structured Pulsate error type returned across crate boundaries.
///
/// Carries a stable [`Code`], a contextual message, and an optional source for
/// the error chain. Construction never allocates a stack trace; the request
/// path maps this to a response in the [Recover] phase.
#[derive(Debug, thiserror::Error)]
pub struct PulsateError {
    code: Code,
    message: String,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl PulsateError {
    /// Build an error from a code and a message.
    #[must_use]
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    /// Attach an underlying source error to the chain.
    #[must_use]
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The stable code.
    #[must_use]
    pub fn code(&self) -> Code {
        self.code
    }

    /// The error category.
    #[must_use]
    pub fn category(&self) -> Category {
        self.code.category
    }

    /// The contextual message. For `Internal`-category errors this stays in the
    /// logs and is never placed in a client-facing body.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The request-facing HTTP status for this error.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        self.code.http_status()
    }
}

impl fmt::Display for PulsateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.code, self.code.title(), self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_renders_zero_padded() {
        assert_eq!(Code::PRX_NO_HEALTHY.to_string(), "PLS-PRX-0003");
        assert_eq!(Code::CFG_SYNTAX.to_string(), "PLS-CFG-0001");
    }

    #[test]
    fn status_override_beats_category_default() {
        // Security default is 403, but a rate limit overrides to 429.
        assert_eq!(Code::SEC_RATE_LIMIT.http_status(), 429);
        // No override: falls back to the category default.
        assert_eq!(
            Code::TLS_NO_CERT.http_status(),
            Category::Security.default_status()
        );
    }

    #[test]
    fn all_codes_are_unique() {
        let all = Code::all();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.to_string(), b.to_string(), "duplicate code {a}");
            }
        }
    }

    #[test]
    fn docs_url_is_well_formed() {
        assert_eq!(
            Code::PRX_NO_HEALTHY.docs_url(),
            "https://squaretick.dev/pulsate/errors/PLS-PRX-0003"
        );
    }

    #[test]
    fn error_displays_with_code_and_message() {
        let e = PulsateError::new(Code::PRX_NO_HEALTHY, "no healthy target in @api");
        assert!(e.to_string().contains("PLS-PRX-0003"));
        assert_eq!(e.http_status(), 503);
        assert_eq!(e.category(), Category::Upstream);
    }
}
