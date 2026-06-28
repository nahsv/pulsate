//! `pulsate-control` — the control-plane orchestrator and REST admin API.
//!
//! A small, loopback-by-default HTTP surface with bearer-token auth and RBAC
//! scopes (`docs/22-admin-api.md`). It exposes runtime state (info, metrics,
//! upstreams, audit), config validation/reload, and serves the embedded
//! dashboard. Every privileged action checks a scope; errors are
//! `application/problem+json` keyed to `PLS-ADM-*` codes.
//!
//! The gRPC surface and event streams are not implemented.
#![forbid(unsafe_code)]

mod json;
mod serve;

use std::collections::HashMap;
use std::sync::Arc;

use pulsate_config::ConfigStore;
use pulsate_http::Gateway;
use pulsate_waf::AuditLog;

#[doc(inline)]
pub use serve::serve_admin;

/// RBAC scopes a token may hold.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scopes {
    /// Read runtime state (info, metrics, upstreams).
    pub read: bool,
    /// Mutate config (validate, reload).
    pub write: bool,
    /// Administrative actions (audit log, tokens).
    pub admin: bool,
}

impl Scopes {
    /// A token with every scope.
    #[must_use]
    pub fn all() -> Self {
        Self {
            read: true,
            write: true,
            admin: true,
        }
    }

    fn satisfies(self, required: Scope) -> bool {
        match required {
            Scope::Read => self.read,
            Scope::Write => self.write,
            Scope::Admin => self.admin,
        }
    }
}

/// A single required scope for an endpoint.
#[derive(Debug, Clone, Copy)]
enum Scope {
    Read,
    Write,
    Admin,
}

/// The admin API: the state and token table the request handlers read.
pub struct AdminApi {
    store: Arc<ConfigStore>,
    gateway: Arc<Gateway>,
    audit: Arc<AuditLog>,
    tokens: HashMap<String, Scopes>,
}

impl AdminApi {
    /// Build an admin API. `admin_token` is granted every scope; pass it as
    /// `Authorization: Bearer <token>`.
    #[must_use]
    pub fn new(
        store: Arc<ConfigStore>,
        gateway: Arc<Gateway>,
        audit: Arc<AuditLog>,
        admin_token: impl Into<String>,
    ) -> Self {
        let mut tokens = HashMap::new();
        tokens.insert(admin_token.into(), Scopes::all());
        Self {
            store,
            gateway,
            audit,
            tokens,
        }
    }

    /// Register an additional token with specific scopes.
    pub fn add_token(&mut self, token: impl Into<String>, scopes: Scopes) {
        self.tokens.insert(token.into(), scopes);
    }

    /// Resolve the scopes for a bearer token, if known.
    fn scopes_for(&self, token: &str) -> Option<Scopes> {
        self.tokens.get(token).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_gate_by_kind() {
        let read_only = Scopes {
            read: true,
            ..Scopes::default()
        };
        assert!(read_only.satisfies(Scope::Read));
        assert!(!read_only.satisfies(Scope::Write));
        assert!(Scopes::all().satisfies(Scope::Admin));
    }
}
