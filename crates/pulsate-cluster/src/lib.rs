//! `pulsate-cluster` — membership, leader election, and replicated counters.
//!
//! The cluster *state model* (`docs/32-disaster-recovery-and-ha.md`): a
//! [`Membership`] view, deterministic [`leader`](Membership::leader) selection
//! (so exactly one node drives shared cert issuance without a coordination
//! round-trip), and a grow-only [`GCounter`] CRDT that lets nodes merge
//! distributed rate-limit / cache counts without locks or a central authority.
//!
//! The gossip transport is not implemented.
#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};

/// A stable node identifier within the cluster.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(String);

impl NodeId {
    /// Wrap a node identifier string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A node's view of cluster membership.
#[derive(Debug, Clone)]
pub struct Membership {
    me: NodeId,
    nodes: BTreeSet<NodeId>,
}

impl Membership {
    /// Start a membership view containing only this node.
    #[must_use]
    pub fn new(me: NodeId) -> Self {
        let mut nodes = BTreeSet::new();
        nodes.insert(me.clone());
        Self { me, nodes }
    }

    /// Add a peer to the view.
    pub fn join(&mut self, node: NodeId) {
        self.nodes.insert(node);
    }

    /// Remove a peer (e.g. on a failure detector timeout).
    pub fn leave(&mut self, node: &NodeId) {
        if node != &self.me {
            self.nodes.remove(node);
        }
    }

    /// The number of known nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the view is empty (never true — it always contains `me`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// All known nodes, in sorted order.
    #[must_use]
    pub fn members(&self) -> Vec<&NodeId> {
        self.nodes.iter().collect()
    }

    /// The deterministically-elected leader: the lowest node id. Every node that
    /// shares the same membership view computes the same leader without
    /// coordination.
    #[must_use]
    pub fn leader(&self) -> Option<&NodeId> {
        self.nodes.iter().next()
    }

    /// Whether this node is currently the leader.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        self.leader() == Some(&self.me)
    }
}

/// A grow-only counter CRDT: each node owns a slot it increments; the merged
/// value is the sum of the per-node maxima. Merges are commutative, associative,
/// and idempotent, so nodes can exchange state in any order and converge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GCounter {
    counts: HashMap<NodeId, u64>,
}

impl GCounter {
    /// An empty counter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment this node's slot by `n`.
    pub fn incr(&mut self, node: &NodeId, n: u64) {
        *self.counts.entry(node.clone()).or_insert(0) += n;
    }

    /// The merged total across all nodes.
    #[must_use]
    pub fn value(&self) -> u64 {
        self.counts.values().sum()
    }

    /// Merge another counter into this one, taking the per-node maximum.
    pub fn merge(&mut self, other: &GCounter) {
        for (node, &count) in &other.counts {
            let slot = self.counts.entry(node.clone()).or_insert(0);
            *slot = (*slot).max(count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> NodeId {
        NodeId::new(s)
    }

    #[test]
    fn leader_is_deterministic_lowest_id() {
        let mut m = Membership::new(n("node-b"));
        m.join(n("node-a"));
        m.join(n("node-c"));
        assert_eq!(m.leader(), Some(&n("node-a")));
        assert!(!m.is_leader());

        // A node that *is* the lowest leads.
        let solo = Membership::new(n("node-a"));
        assert!(solo.is_leader());
    }

    #[test]
    fn membership_join_and_leave() {
        let mut m = Membership::new(n("a"));
        m.join(n("b"));
        assert_eq!(m.len(), 2);
        m.leave(&n("b"));
        assert_eq!(m.len(), 1);
        // A node cannot evict itself.
        m.leave(&n("a"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn gcounter_merges_commutatively_and_idempotently() {
        let (a, b) = (n("a"), n("b"));
        let mut x = GCounter::new();
        x.incr(&a, 3);
        let mut y = GCounter::new();
        y.incr(&b, 5);
        y.incr(&a, 1); // a stale lower view of a's slot

        let mut merged = x.clone();
        merged.merge(&y);
        assert_eq!(merged.value(), 8); // max(a:3,1)=3 + b:5

        // Idempotent: merging again changes nothing.
        let before = merged.clone();
        merged.merge(&y);
        assert_eq!(merged, before);

        // Commutative: y.merge(x) reaches the same value.
        let mut other = y.clone();
        other.merge(&x);
        assert_eq!(other.value(), merged.value());
    }
}
