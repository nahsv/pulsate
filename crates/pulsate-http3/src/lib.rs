//! `pulsate-http3` — HTTP/3 discovery.
//!
//! The discovery half of HTTP/3: the `Alt-Svc` advertisement that tells clients
//! an `h3` endpoint is available, plus the typed config a listener uses to enable
//! it (`docs/05-http-stack.md`). When `h3` is enabled, the HTTP/1 and HTTP/2
//! responses carry `Alt-Svc: h3=":<port>"`, so a compliant client upgrades its
//! *next* connection to QUIC.
//!
//! The QUIC/h3 transport itself is not implemented.
#![forbid(unsafe_code)]

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
