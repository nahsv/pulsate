//! The immutable configuration snapshot.
//!
//! [`ConfigSnapshot`] is the single object that crosses the control-plane →
//! data-plane boundary. The control plane builds one from a validated config
//! and publishes it via `arc-swap`; the data plane loads an `Arc` of it per
//! request and runs to completion against that generation, lock-free
//! (`docs/02-architecture.md#hot-reload-architecture`).
//!
//! A snapshot carries its identity and generation; the compiled routing table,
//! resolved upstreams, and cert view attach through the builder.

use std::sync::Arc;

/// A content-addressable identity for a snapshot.
///
/// Two snapshots built from equivalent configs hash equal, which makes reloads
/// diffable and auditable. The 64-bit digest
/// is supplied by the caller; the config builder derives it from the
/// canonicalized config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotId(u64);

impl SnapshotId {
    /// Wrap a precomputed content digest.
    #[must_use]
    pub const fn from_digest(digest: u64) -> Self {
        Self(digest)
    }

    /// The raw digest value.
    #[must_use]
    pub const fn digest(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// An immutable, atomically-published view of the running configuration.
///
/// Everything reachable from a snapshot is immutable and shared by `Arc`, so
/// data-plane reads are pointer-chases, never locks. Cloning is cheap: it is
/// `Arc`-shaped internally. Build via [`ConfigSnapshot::builder`].
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    inner: Arc<SnapshotInner>,
}

#[derive(Debug)]
struct SnapshotInner {
    id: SnapshotId,
    /// Monotonic generation counter, incremented on every successful publish.
    /// Lets the control plane keep exactly one prior generation for rollback.
    generation: u64,
}

impl ConfigSnapshot {
    /// Start building a snapshot with the given identity and generation.
    #[must_use]
    pub fn builder(id: SnapshotId, generation: u64) -> SnapshotBuilder {
        SnapshotBuilder { id, generation }
    }

    /// The snapshot's content identity.
    #[must_use]
    pub fn id(&self) -> SnapshotId {
        self.inner.id
    }

    /// The monotonic generation number.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }
}

/// Builder for a [`ConfigSnapshot`]. Subsystem views (routes, upstreams, certs)
/// are added through this builder.
#[derive(Debug)]
pub struct SnapshotBuilder {
    id: SnapshotId,
    generation: u64,
}

impl SnapshotBuilder {
    /// Finalize the immutable snapshot.
    #[must_use]
    pub fn build(self) -> ConfigSnapshot {
        ConfigSnapshot {
            inner: Arc::new(SnapshotInner {
                id: self.id,
                generation: self.generation,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_carries_identity_and_generation() {
        let snap = ConfigSnapshot::builder(SnapshotId::from_digest(0xDEAD_BEEF), 7).build();
        assert_eq!(snap.id().digest(), 0xDEAD_BEEF);
        assert_eq!(snap.generation(), 7);
    }

    #[test]
    fn clone_shares_the_same_inner() {
        let a = ConfigSnapshot::builder(SnapshotId::from_digest(1), 1).build();
        let b = a.clone();
        assert_eq!(a.id(), b.id());
        // Clone is Arc-shaped: pointer identity is preserved.
        assert!(Arc::ptr_eq(&a.inner, &b.inner));
    }

    #[test]
    fn id_renders_fixed_width_hex() {
        assert_eq!(
            SnapshotId::from_digest(0xABC).to_string(),
            "0000000000000abc"
        );
    }
}
