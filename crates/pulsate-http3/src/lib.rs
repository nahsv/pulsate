//! `pulsate-http3` — HTTP/3 discovery **and** a QUIC/HTTP-3 transport.
//!
//! Two halves of HTTP/3 live here:
//!
//! 1. **Discovery** ([`Http3Config`]): the `Alt-Svc` advertisement that tells
//!    clients an `h3` endpoint is available. When `h3` is enabled the HTTP/1 and
//!    HTTP/2 responses carry `Alt-Svc: h3=":<port>"`, so a compliant client
//!    upgrades its *next* connection to QUIC (`docs/05-http-stack.md`).
//! 2. **Transport** ([`Http3Listener`]): a `quinn` UDP endpoint speaking QUIC +
//!    HTTP/3 (`h3`/`h3-quinn`). Each request is bridged through the same routing,
//!    middleware, cache, and proxy core the HTTP/1 and HTTP/2 listeners use, so
//!    behaviour is identical across protocols.
//!
//! 0-RTT (early data) is intentionally disabled; see [`server`] for the rationale.
#![forbid(unsafe_code)]

pub mod dispatch;
pub mod server;

#[doc(inline)]
pub use server::{Http3Error, Http3Listener, TransportConfig};

/// HTTP/3 listener configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Http3Config {
    /// Whether HTTP/3 is enabled.
    pub enabled: bool,
    /// The UDP port the QUIC endpoint advertises (usually the HTTPS port).
    pub port: u16,
    /// `Alt-Svc` max-age in seconds.
    pub max_age_secs: u32,
}

impl Default for Http3Config {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 443,
            max_age_secs: 86_400,
        }
    }
}

impl Http3Config {
    /// Enable HTTP/3 on `port`.
    #[must_use]
    pub fn enabled(port: u16) -> Self {
        Self {
            enabled: true,
            port,
            ..Self::default()
        }
    }

    /// The `Alt-Svc` header value advertising this endpoint, or `None` when
    /// disabled.
    #[must_use]
    pub fn alt_svc(&self) -> Option<String> {
        self.enabled
            .then(|| format!("h3=\":{}\"; ma={}", self.port, self.max_age_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_has_no_alt_svc() {
        assert!(Http3Config::default().alt_svc().is_none());
    }

    #[test]
    fn enabled_advertises_h3() {
        let cfg = Http3Config::enabled(8443);
        assert_eq!(cfg.alt_svc().as_deref(), Some("h3=\":8443\"; ma=86400"));
    }
}
