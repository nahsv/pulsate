//! `pulsate-acme` — the certificate-management plumbing for automatic TLS.
//!
//! The pieces the ACME lifecycle is built on (`docs/09-security.md`): an
//! [`Http01Store`] holding the token → key-authorization mapping the data plane
//! answers HTTP-01 challenges from, a [`DynamicCertStore`] whose certificates can
//! be replaced live as they are issued and renewed (it implements rustls'
//! `ResolvesServerCert`, so it slots straight into a server config), and an
//! [`OnDemandPolicy`] allow-list gating on-demand issuance.
//!
//! Live issuance against a CA is not wired up.
#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

/// The well-known path prefix for HTTP-01 challenge responses.
pub const HTTP01_PREFIX: &str = "/.well-known/acme-challenge/";

/// A concurrent store of in-flight HTTP-01 challenges.
///
/// The ACME client registers `token → key_authorization` before asking the CA
/// to validate; the data plane serves `GET {HTTP01_PREFIX}{token}` with the
/// stored key authorization. Cheap to clone (`Arc`-backed).
#[derive(Debug, Clone, Default)]
pub struct Http01Store {
    inner: Arc<Mutex<HashMap<String, String>>>,
}

impl Http01Store {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a challenge response.
    pub fn set(&self, token: impl Into<String>, key_authorization: impl Into<String>) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(token.into(), key_authorization.into());
        }
    }

    /// Look up the key authorization for a challenge path or token.
    #[must_use]
    pub fn get(&self, token_or_path: &str) -> Option<String> {
        let token = token_or_path
            .strip_prefix(HTTP01_PREFIX)
            .unwrap_or(token_or_path);
        self.inner.lock().ok()?.get(token).cloned()
    }

    /// Remove a completed challenge.
    pub fn remove(&self, token: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(token);
        }
    }
}

/// A certificate store whose entries can be replaced at runtime as certs are
/// issued and renewed. Implements `ResolvesServerCert` for direct use in a
/// rustls `ServerConfig`.
#[derive(Debug, Default)]
pub struct DynamicCertStore {
    by_host: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl DynamicCertStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install (or replace) the certificate for `host` from PEM.
    ///
    /// # Errors
    /// Returns an error string if the PEM is invalid.
    pub fn install_pem(&self, host: &str, cert_pem: &[u8], key_pem: &[u8]) -> Result<(), String> {
        let ck =
            pulsate_tls::certified_key_from_pem(cert_pem, key_pem).map_err(|e| e.to_string())?;
        if let Ok(mut map) = self.by_host.lock() {
            map.insert(host.to_string(), Arc::new(ck));
        }
        Ok(())
    }

    /// Whether a certificate exists for `host`.
    #[must_use]
    pub fn has(&self, host: &str) -> bool {
        self.by_host.lock().is_ok_and(|m| m.contains_key(host))
    }
}

impl ResolvesServerCert for DynamicCertStore {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = client_hello.server_name()?;
        self.by_host.lock().ok()?.get(name).cloned()
    }
}

/// An allow-list controlling which hostnames may trigger on-demand issuance,
/// so an attacker cannot make Pulsate request certificates for arbitrary SNI
/// values (`docs/09-security.md`, `PLS-ACME-0004`).
#[derive(Debug, Clone, Default)]
pub struct OnDemandPolicy {
    allowed: HashSet<String>,
    allow_any: bool,
}

impl OnDemandPolicy {
    /// A policy that denies everything until hosts are added.
    #[must_use]
    pub fn deny_all() -> Self {
        Self::default()
    }

    /// A policy that allows any host (use only behind another gate).
    #[must_use]
    pub fn allow_any() -> Self {
        Self {
            allowed: HashSet::new(),
            allow_any: true,
        }
    }

    /// Allow a specific host.
    #[must_use]
    pub fn allow(mut self, host: impl Into<String>) -> Self {
        self.allowed.insert(host.into());
        self
    }

    /// Whether `host` may trigger on-demand issuance.
    #[must_use]
    pub fn allows(&self, host: &str) -> bool {
        self.allow_any || self.allowed.contains(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn self_signed(host: &str) -> (String, String) {
        let cert = rcgen::generate_simple_self_signed(vec![host.to_string()]).unwrap();
        (cert.cert.pem(), cert.key_pair.serialize_pem())
    }

    #[test]
    fn http01_store_matches_token_and_path() {
        let store = Http01Store::new();
        store.set("tok123", "tok123.keyauth");
        assert_eq!(store.get("tok123").as_deref(), Some("tok123.keyauth"));
        // Lookup by full challenge path also works.
        assert_eq!(
            store.get("/.well-known/acme-challenge/tok123").as_deref(),
            Some("tok123.keyauth")
        );
        store.remove("tok123");
        assert!(store.get("tok123").is_none());
    }

    #[test]
    fn dynamic_cert_store_installs_and_reports() {
        let (cert, key) = self_signed("live.example.com");
        let store = DynamicCertStore::new();
        assert!(!store.has("live.example.com"));
        store
            .install_pem("live.example.com", cert.as_bytes(), key.as_bytes())
            .unwrap();
        assert!(store.has("live.example.com"));
        assert!(store.install_pem("x", b"bad", b"bad").is_err());
    }

    #[test]
    fn on_demand_policy_gates_hosts() {
        let policy = OnDemandPolicy::deny_all().allow("shop.example.com");
        assert!(policy.allows("shop.example.com"));
        assert!(!policy.allows("evil.example.com"));
        assert!(OnDemandPolicy::allow_any().allows("anything.com"));
    }
}
