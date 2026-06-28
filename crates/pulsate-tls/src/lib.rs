//! `pulsate-tls` — rustls server-side TLS configuration.
//!
//! Builds a [`rustls::ServerConfig`] that selects a certificate by SNI from a
//! resolver and negotiates the protocol via ALPN. Constructed explicitly over
//! the `ring` crypto provider, so it carries no global process state.
//! Certificates load from PEM; any other source plugs into the same
//! [`CertResolver`]. Client auth is not configured (`docs/09-security.md`).
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;

/// Error building a TLS configuration or loading a certificate.
#[derive(Debug)]
pub struct TlsError(String);

impl TlsError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tls error: {}", self.0)
    }
}

impl std::error::Error for TlsError {}

/// Parse a certificate chain + private key (both PEM) into a [`CertifiedKey`].
///
/// # Errors
/// Returns [`TlsError`] if the PEM is malformed, the chain is empty, or the key
/// type is unsupported by the `ring` provider.
pub fn certified_key_from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<CertifiedKey, TlsError> {
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<Result<_, _>>()
        .map_err(|e| TlsError::new(format!("invalid certificate PEM: {e}")))?;
    if certs.is_empty() {
        return Err(TlsError::new("no certificates found in PEM"));
    }

    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut &key_pem[..])
        .map_err(|e| TlsError::new(format!("invalid key PEM: {e}")))?
        .ok_or_else(|| TlsError::new("no private key found in PEM"))?;

    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| TlsError::new(format!("unsupported key type: {e}")))?;

    Ok(CertifiedKey::new(certs, signing_key))
}

/// Resolves a server certificate by SNI server name, with a default fallback.
#[derive(Debug, Default)]
pub struct CertResolver {
    by_host: HashMap<String, Arc<CertifiedKey>>,
    default: Option<Arc<CertifiedKey>>,
}

impl CertResolver {
    /// An empty resolver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a certificate for an exact SNI host name.
    pub fn insert(&mut self, host: impl Into<String>, key: CertifiedKey) {
        self.by_host.insert(host.into(), Arc::new(key));
    }

    /// Set the fallback certificate used when SNI is absent or unmatched.
    pub fn set_default(&mut self, key: CertifiedKey) {
        self.default = Some(Arc::new(key));
    }

    /// Whether the resolver can serve any certificate at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_host.is_empty() && self.default.is_none()
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        client_hello
            .server_name()
            .and_then(|name| self.by_host.get(name))
            .or(self.default.as_ref())
            .map(Arc::clone)
    }
}

/// Build a server config from a populated [`CertResolver`], advertising HTTP/2
/// and HTTP/1.1 via ALPN. Built explicitly over the `ring` provider.
///
/// # Errors
/// Returns [`TlsError`] if the protocol-version configuration is rejected.
pub fn server_config(resolver: CertResolver) -> Result<Arc<ServerConfig>, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| TlsError::new(format!("protocol version setup failed: {e}")))?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    // Advertise HTTP/2 then HTTP/1.1 via ALPN; hyper's auto server negotiates.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// A TLS acceptor wrapping a server config, ready to handshake accepted streams.
#[must_use]
pub fn acceptor(config: Arc<ServerConfig>) -> tokio_rustls::TlsAcceptor {
    tokio_rustls::TlsAcceptor::from(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a self-signed cert/key PEM pair for `host`.
    fn self_signed(host: &str) -> (String, String) {
        let cert = rcgen::generate_simple_self_signed(vec![host.to_string()]).unwrap();
        (cert.cert.pem(), cert.key_pair.serialize_pem())
    }

    #[test]
    fn loads_pem_and_builds_config() {
        let (cert_pem, key_pem) = self_signed("localhost");
        let ck = certified_key_from_pem(cert_pem.as_bytes(), key_pem.as_bytes()).unwrap();

        let mut resolver = CertResolver::new();
        resolver.insert("localhost", ck);
        assert!(!resolver.is_empty());

        let config = server_config(resolver).unwrap();
        assert_eq!(
            config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn rejects_empty_certificate_pem() {
        let err = certified_key_from_pem(b"not a pem", b"also not");
        assert!(err.is_err());
    }
}
