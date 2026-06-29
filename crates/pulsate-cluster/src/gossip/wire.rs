//! Versioned wire format for the SWIM gossip transport.
//!
//! Frames are encoded with [`postcard`] (a compact, `no_std`-friendly binary
//! serde format) and carried in single UDP datagrams. Every frame begins with a
//! [`PROTOCOL_VERSION`] byte; a receiver drops any frame whose version does not
//! match its own, so a future incompatible format can be introduced by bumping
//! the constant without misinterpreting old peers' bytes.
//!
//! Each frame also piggy-backs an anti-entropy payload — the sender's full view
//! of membership ([`MemberUpdate`] entries) plus its entire
//! [`GCounter`](crate::GCounter) — so that *any* received frame, regardless of
//! [`Kind`], reconciles state and converges the cluster.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::{GCounter, NodeId};

/// The on-wire protocol version. Frames carrying a different value are dropped
/// rather than decoded, guaranteeing forward safety across upgrades.
pub const PROTOCOL_VERSION: u8 = 1;

/// The failure-detection state of a member.
///
/// States are totally ordered by [`MemberState::rank`] — `Alive` < `Suspect` <
/// `Dead` — which breaks ties when two updates share the same incarnation:
/// stronger evidence of failure always wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberState {
    /// The member is responding to probes (directly or indirectly).
    Alive,
    /// A probe to the member failed; it is provisionally failed pending a
    /// refutation or the suspicion timeout.
    Suspect,
    /// The member is confirmed failed. Kept as a tombstone and never reaped, so
    /// the death keeps propagating and the node cannot silently rejoin.
    Dead,
}

impl MemberState {
    /// Precedence rank used to break ties at equal incarnation: `Dead` (2)
    /// beats `Suspect` (1) beats `Alive` (0).
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            MemberState::Alive => 0,
            MemberState::Suspect => 1,
            MemberState::Dead => 2,
        }
    }
}

/// A single anti-entropy entry: one member's advertised address, incarnation
/// number, and failure-detection state.
///
/// The incarnation is a per-node logical clock that the node alone increments
/// (to refute a false suspicion). Reconciliation prefers the higher incarnation
/// and, at equal incarnation, the higher [`MemberState::rank`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberUpdate {
    /// The member this entry describes.
    pub node: NodeId,
    /// The member's advertised (resolved) UDP address.
    pub addr: SocketAddr,
    /// The member's incarnation number as known to the sender.
    pub incarnation: u64,
    /// The member's failure-detection state as known to the sender.
    pub state: MemberState,
}

/// The kind of a gossip frame — the SWIM message type it carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Kind {
    /// A direct liveness probe; the receiver replies with [`Kind::Ack`] reusing
    /// the frame's sequence number.
    Ping,
    /// A reply to a [`Kind::Ping`] (or a relayed indirect probe), echoing the
    /// probe's sequence number.
    Ack,
    /// An indirect-probe request: "please ping `target` on my behalf and relay
    /// its ack back to me." Sent to helper peers when a direct ping times out.
    PingReq {
        /// The address the recipient should probe on the origin's behalf.
        target: SocketAddr,
    },
}

/// A versioned gossip datagram.
///
/// Beyond the [`Kind`]-specific routing fields it always carries the sender's
/// identity, a monotonically increasing `seq` (used to correlate acks with
/// probes), and the anti-entropy payload (`updates` + `counter`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    /// Protocol version; checked against [`PROTOCOL_VERSION`] before use.
    pub version: u8,
    /// The sending node's identifier.
    pub from: NodeId,
    /// The sending node's advertised (resolved) UDP address.
    pub from_addr: SocketAddr,
    /// Probe sequence number; an ack echoes the ping's value.
    pub seq: u64,
    /// The message type and its routing fields.
    pub kind: Kind,
    /// The sender's full membership view (anti-entropy).
    pub updates: Vec<MemberUpdate>,
    /// The sender's full grow-only counter (anti-entropy).
    pub counter: GCounter,
}

impl Frame {
    /// Serialize the frame to bytes for a single UDP datagram.
    pub fn encode(&self) -> Result<Vec<u8>, postcard::Error> {
        postcard::to_allocvec(self)
    }

    /// Decode a frame from received datagram bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}
