//! `pulsate-control` — the control-plane orchestrator and REST admin API.
//!
//! A small, loopback-by-default HTTP surface with bearer-token auth and RBAC
//! scopes (`docs/22-admin-api.md`). It exposes runtime state (info, metrics,
//! upstreams, audit), config validation/reload, and serves the embedded
//! dashboard. Every privileged action checks a scope; errors are
//! `application/problem+json` keyed to `PLS-ADM-*` codes.
//!
//! A parallel gRPC surface ([`serve_grpc`]) exposes the same operations over
//! `tonic`, plus a server-streaming event feed ([`AdminEvent`]). Both surfaces
//! share one [`AdminApi`] — the same token table, RBAC scopes, and event bus.
#![forbid(unsafe_code)]

mod grpc;
mod json;
mod serve;

/// Generated `tonic`/`prost` types for the `pulsate.admin.v1` service.
///
/// This module is machine-generated, so the crate's pedantic lints and the
/// missing-docs gate are relaxed for its contents only.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    missing_docs,
    unreachable_pub,
    clippy::derive_partial_eq_without_eq
)]
mod proto {
    tonic::include_proto!("pulsate.admin.v1");
}

use std::collections::HashMap;
use std::sync::Arc;

use pulsate_config::ConfigStore;
use pulsate_http::Gateway;
use pulsate_waf::AuditLog;
use tokio::sync::broadcast;

#[doc(inline)]
pub use grpc::serve_grpc;
#[doc(inline)]
pub use serve::serve_admin;

/// An event published on the admin event bus and surfaced over `WatchEvents`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AdminEvent {
    /// The configuration was reloaded to a new generation.
    ConfigReloaded {
        /// The newly-published generation number.
        generation: u64,
    },
    /// The process lifecycle state changed.
    LifecycleChanged {
        /// The new lifecycle state, rendered as a string.
        state: String,
    },
    /// A new entry was appended to the audit log.
    AuditAppended {
        /// The entry's sequence number.
        seq: u64,
        /// The event description.
        event: String,
        /// The entry's hash, hex-rendered.
        hash: String,
    },
}

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
    events: broadcast::Sender<AdminEvent>,
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
        let (events, _) = broadcast::channel(128);
        Self {
            store,
            gateway,
            audit,
            tokens,
            events,
        }
    }

    /// Register an additional token with specific scopes.
    pub fn add_token(&mut self, token: impl Into<String>, scopes: Scopes) {
        self.tokens.insert(token.into(), scopes);
    }

    /// Resolve the scopes for a bearer token, if known.
    ///
    /// Compares the presented token against every registered token in
    /// constant time over their SHA-256 digests, so neither a match/no-match
    /// decision nor token length leaks through response timing (LOW).
    fn scopes_for(&self, token: &str) -> Option<Scopes> {
        let presented = ring::digest::digest(&ring::digest::SHA256, token.as_bytes());
        let mut found = None;
        for (known, scopes) in &self.tokens {
            let known_digest = ring::digest::digest(&ring::digest::SHA256, known.as_bytes());
            if constant_time_eq::constant_time_eq(presented.as_ref(), known_digest.as_ref()) {
                found = Some(*scopes);
            }
        }
        found
    }

    /// Subscribe to the admin event bus. Each subscriber receives every event
    /// published after it subscribed; slow consumers may observe lag.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AdminEvent> {
        self.events.subscribe()
    }

    /// Publish an event to every current subscriber. Dropped if there are none.
    pub fn publish(&self, event: AdminEvent) {
        let _ = self.events.send(event);
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
