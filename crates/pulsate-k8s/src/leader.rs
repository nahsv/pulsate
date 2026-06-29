//! Leader-election gating for the reconciler.
//!
//! Every replica of a Pulsate deployment runs the same controller, but only the
//! elected leader should write config and resource status — otherwise replicas
//! fight over the same `ConfigStore` and Kubernetes status sub-resources. Rather
//! than bake in one election mechanism, the reconciler is gated behind the
//! [`LeaderGuard`] trait so the election source is pluggable.
//!
//! The default [`AlwaysLeader`] guard always returns `true`, which is correct for
//! a single-replica deployment and keeps the cluster-free tests simple. In a
//! multi-replica deployment, wire in a guard backed by the `pulsate-cluster`
//! gossip membership (its `Membership::is_leader`) or a Kubernetes `Lease`-based
//! elector. **Until such a guard is supplied, running multiple replicas with
//! `AlwaysLeader` is unsafe** — see the deferred-work note in the crate docs.

/// A pluggable "am I the leader right now?" check.
///
/// Implementations must be cheap and non-blocking: the reconciler calls
/// [`is_leader`](LeaderGuard::is_leader) on every reconcile pass.
pub trait LeaderGuard: Send + Sync + 'static {
    /// Whether this replica currently holds leadership and may mutate cluster
    /// and config state.
    fn is_leader(&self) -> bool;
}

/// The default guard: this replica is always the leader.
///
/// Correct for single-replica deployments. For multi-replica deployments,
/// replace it with a real elector (see the module docs).
#[derive(Clone, Copy, Debug, Default)]
pub struct AlwaysLeader;

impl LeaderGuard for AlwaysLeader {
    fn is_leader(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{AlwaysLeader, LeaderGuard};

    /// A guard whose leadership can be toggled, used to prove gating works.
    struct Toggle(bool);
    impl LeaderGuard for Toggle {
        fn is_leader(&self) -> bool {
            self.0
        }
    }

    #[test]
    fn always_leader_is_always_leader() {
        assert!(AlwaysLeader.is_leader());
    }

    #[test]
    fn toggle_guard_reports_its_state() {
        assert!(Toggle(true).is_leader());
        assert!(!Toggle(false).is_leader());
    }
}
