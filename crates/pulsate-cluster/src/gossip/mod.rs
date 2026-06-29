//! Live SWIM-style gossip transport driving [`Membership`](crate::Membership)
//! and [`GCounter`](crate::GCounter) over UDP.
//!
//! # Protocol
//!
//! Each node owns one [`tokio::net::UdpSocket`]. A single event loop interleaves
//! receiving frames with three periodic duties on every `probe_interval`:
//!
//! * **Failure detection.** One peer is probed per round with a direct
//!   [`Ping`](wire::Kind::Ping). If no [`Ack`](wire::Kind::Ack) arrives within
//!   `probe_timeout`, the prober asks `indirect_probes` other peers to
//!   [`PingReq`](wire::Kind::PingReq) the target; a relay that gets the target's
//!   ack forwards it back to the origin, **reusing the original probe sequence
//!   number** so the origin recognizes it. Only if both the direct and indirect
//!   phases fail does the target become `Suspect`.
//! * **Suspicion FSM.** `Alive → Suspect → Dead`. A node confirmed `Dead` after
//!   `suspicion_timeout` is removed from the live [`Membership`] view via
//!   [`Membership::leave`](crate::Membership::leave) but kept as a tombstone so
//!   the death keeps propagating. Learning a peer is `Alive` calls
//!   [`Membership::join`](crate::Membership::join). A node that sees *itself*
//!   suspected or declared dead refutes by bumping its own incarnation and
//!   re-gossiping.
//! * **Anti-entropy.** Every frame piggy-backs the sender's full membership view
//!   and entire [`GCounter`]. Receivers reconcile each member by
//!   `(incarnation, state-rank)` precedence (`Dead` > `Suspect` > `Alive`) and
//!   run [`GCounter::merge`](crate::GCounter::merge), so any message — ping, ack,
//!   or ping-req — drives convergence.

pub mod wire;

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::{GCounter, Membership, NodeId};
use wire::{Frame, Kind, MemberState, MemberUpdate, PROTOCOL_VERSION};

/// Runtime configuration for a [`Cluster`] node.
///
/// Construct with [`Config::new`] for sensible production defaults, then adjust
/// individual fields (the failure-detector timings are deliberately public so
/// tests can drive a tight loopback simulation).
#[derive(Debug, Clone)]
pub struct Config {
    /// The local address to bind the UDP socket to. Use a `:0` port to let the
    /// OS choose; the resolved address becomes this node's identity.
    pub bind: SocketAddr,
    /// Seed peers to gossip with on startup so a new node can discover the
    /// cluster. Their identities are learned from the frames they return.
    pub seeds: Vec<SocketAddr>,
    /// How often a probe round (failure detection + dissemination) runs.
    pub probe_interval: Duration,
    /// How long to await a direct ack before falling back to indirect probes.
    pub probe_timeout: Duration,
    /// How long a member may remain `Suspect` before being confirmed `Dead`.
    pub suspicion_timeout: Duration,
    /// Number of helper peers asked to indirectly probe a target.
    pub indirect_probes: usize,
    /// Number of random peers a node disseminates to each round.
    pub gossip_fanout: usize,
}

impl Config {
    /// A configuration bound to `bind` with production-leaning defaults: a
    /// one-second probe interval, 300 ms direct-probe timeout, three-second
    /// suspicion timeout, three indirect probes, and a gossip fanout of three.
    #[must_use]
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            seeds: Vec::new(),
            probe_interval: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(300),
            suspicion_timeout: Duration::from_secs(3),
            indirect_probes: 3,
            gossip_fanout: 3,
        }
    }
}

/// One peer's record in a node's local view: where it lives, its incarnation,
/// failure state, and (when `Suspect`) the deadline at which it turns `Dead`.
#[derive(Debug, Clone)]
struct Peer {
    addr: SocketAddr,
    incarnation: u64,
    state: MemberState,
    suspect_deadline: Option<Instant>,
}

/// State shared between the public [`Cluster`] handle and the background runtime
/// task, guarded by a [`std::sync::Mutex`]. The mutex is never held across an
/// `.await`; senders snapshot under the lock, drop it, then perform I/O.
struct Shared {
    me: NodeId,
    local_addr: SocketAddr,
    incarnation: u64,
    membership: Membership,
    counter: GCounter,
    peers: HashMap<NodeId, Peer>,
}

impl Shared {
    fn new(me: NodeId, local_addr: SocketAddr) -> Self {
        Self {
            membership: Membership::new(me.clone()),
            me,
            local_addr,
            incarnation: 0,
            counter: GCounter::new(),
            peers: HashMap::new(),
        }
    }

    /// Build an outgoing frame: this node's self-update plus every known peer
    /// (including `Dead` tombstones) and the full counter.
    fn snapshot(&self, seq: u64, kind: Kind) -> Frame {
        let mut updates = Vec::with_capacity(self.peers.len() + 1);
        updates.push(MemberUpdate {
            node: self.me.clone(),
            addr: self.local_addr,
            incarnation: self.incarnation,
            state: MemberState::Alive,
        });
        for (node, peer) in &self.peers {
            updates.push(MemberUpdate {
                node: node.clone(),
                addr: peer.addr,
                incarnation: peer.incarnation,
                state: peer.state,
            });
        }
        Frame {
            version: PROTOCOL_VERSION,
            from: self.me.clone(),
            from_addr: self.local_addr,
            seq,
            kind,
            updates,
            counter: self.counter.clone(),
        }
    }
}

/// The current in-flight failure-detection probe.
struct Probe {
    seq: u64,
    target: NodeId,
    target_addr: SocketAddr,
    phase: ProbePhase,
    deadline: Instant,
}

/// Which half of a probe is outstanding.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbePhase {
    /// Awaiting a direct ack from the target.
    Direct,
    /// Direct ping timed out; awaiting an indirect (relayed) ack.
    Indirect,
}

/// A pending relayed probe: this node pinged `target` on `origin`'s behalf and
/// will forward the target's ack back to `origin`.
struct Relay {
    origin: SocketAddr,
    target: SocketAddr,
    expire: Instant,
}

/// A tiny `xorshift64` PRNG used to pick random gossip/indirect-probe peers
/// without pulling in an external dependency. Seeded with a per-process-unique
/// value so sequence numbers do not collide across nodes.
struct Rng(u64);

impl Rng {
    fn seeded() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| {
                d.as_secs()
                    .wrapping_mul(1_000_000_000)
                    .wrapping_add(u64::from(d.subsec_nanos()))
            });
        let mixed = nanos
            ^ COUNTER
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        Self(mixed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Fisher–Yates shuffle.
    fn shuffle<T>(&mut self, items: &mut [T]) {
        let len = items.len();
        for i in (1..len).rev() {
            let bound = i as u64 + 1;
            let j = usize::try_from(self.next_u64() % bound).unwrap_or(0);
            items.swap(i, j);
        }
    }
}

/// The background gossip runtime. Owns the socket and all protocol-only state;
/// reaches into [`Shared`] under its mutex to read/update membership and counter.
struct Runtime {
    socket: UdpSocket,
    config: Config,
    shared: Arc<Mutex<Shared>>,
    rng: Rng,
    seq: u64,
    probe: Option<Probe>,
    relays: HashMap<u64, Relay>,
    round_robin: usize,
}

impl Runtime {
    fn new(socket: UdpSocket, config: Config, shared: Arc<Mutex<Shared>>) -> Self {
        let mut rng = Rng::seeded();
        let seq = rng.next_u64();
        Self {
            socket,
            config,
            shared,
            rng,
            seq,
            probe: None,
            relays: HashMap::new(),
            round_robin: 0,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn next_seq(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    /// Lock, snapshot a frame, drop the lock, then send it. Encode/send errors
    /// (a closed peer socket, an oversized datagram) are non-fatal for UDP.
    async fn send(&self, addr: SocketAddr, seq: u64, kind: Kind) {
        let frame = {
            let shared = self.lock();
            shared.snapshot(seq, kind)
        };
        if let Ok(bytes) = frame.encode() {
            let _ = self.socket.send_to(&bytes, addr).await;
        }
    }

    /// The event loop. Runs until `shutdown` is notified.
    async fn run(mut self, shutdown: Arc<Notify>) {
        let mut buf = vec![0u8; 64 * 1024];
        self.gossip_round().await;
        let mut next_tick = Instant::now() + self.config.probe_interval;

        loop {
            let now = Instant::now();
            self.reap_suspects(now).await;
            self.advance_probe(now).await;
            if now >= next_tick {
                next_tick = now + self.config.probe_interval;
                self.gossip_round().await;
                self.start_probe(now).await;
            }
            self.relays.retain(|_, r| r.expire > now);

            let deadline = self.next_deadline(next_tick);
            let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));

            tokio::select! {
                () = shutdown.notified() => break,
                () = sleep => {}
                res = self.socket.recv_from(&mut buf) => {
                    if let Ok((len, _src)) = res {
                        self.handle(&buf[..len]).await;
                    }
                }
            }
        }
    }

    /// The earliest instant at which the loop must wake to do timed work.
    fn next_deadline(&self, next_tick: Instant) -> Instant {
        let mut deadline = next_tick;
        if let Some(probe) = &self.probe {
            deadline = deadline.min(probe.deadline);
        }
        for relay in self.relays.values() {
            deadline = deadline.min(relay.expire);
        }
        let shared = self.lock();
        for peer in shared.peers.values() {
            if let Some(when) = peer.suspect_deadline {
                deadline = deadline.min(when);
            }
        }
        deadline
    }

    /// Promote any `Suspect` whose deadline has passed to `Dead`, evicting it
    /// from the live membership view (tombstone retained) and re-gossiping.
    async fn reap_suspects(&mut self, now: Instant) {
        let mut died = false;
        {
            let mut shared = self.lock();
            let mut dead = Vec::new();
            for (node, peer) in &mut shared.peers {
                if peer.state == MemberState::Suspect
                    && peer.suspect_deadline.is_some_and(|d| d <= now)
                {
                    peer.state = MemberState::Dead;
                    peer.suspect_deadline = None;
                    dead.push(node.clone());
                }
            }
            for node in dead {
                shared.membership.leave(&node);
                died = true;
            }
        }
        if died {
            self.gossip_round().await;
        }
    }

    /// Drive the in-flight probe through its direct → indirect → suspect phases.
    async fn advance_probe(&mut self, now: Instant) {
        let Some(probe) = self.probe.take() else {
            return;
        };
        if probe.deadline > now {
            self.probe = Some(probe);
            return;
        }
        match probe.phase {
            ProbePhase::Direct => {
                for addr in self.pick_indirect(probe.target_addr) {
                    self.send(
                        addr,
                        probe.seq,
                        Kind::PingReq {
                            target: probe.target_addr,
                        },
                    )
                    .await;
                }
                self.probe = Some(Probe {
                    phase: ProbePhase::Indirect,
                    deadline: now + self.config.probe_timeout,
                    ..probe
                });
            }
            ProbePhase::Indirect => {
                self.mark_suspect(&probe.target, now);
                self.gossip_round().await;
            }
        }
    }

    /// Begin a new direct probe against the next eligible peer (round-robin over
    /// `Alive`/`Suspect` members), unless one is already in flight.
    async fn start_probe(&mut self, now: Instant) {
        if self.probe.is_some() {
            return;
        }
        let mut candidates: Vec<(NodeId, SocketAddr)> = {
            let shared = self.lock();
            shared
                .peers
                .iter()
                .filter(|(_, p)| p.state != MemberState::Dead)
                .map(|(n, p)| (n.clone(), p.addr))
                .collect()
        };
        let target = if candidates.is_empty() {
            None
        } else {
            candidates.sort_by(|a, b| a.0.cmp(&b.0));
            let idx = self.round_robin % candidates.len();
            self.round_robin = self.round_robin.wrapping_add(1);
            Some(candidates.swap_remove(idx))
        };
        if let Some((node, addr)) = target {
            let seq = self.next_seq();
            self.send(addr, seq, Kind::Ping).await;
            self.probe = Some(Probe {
                seq,
                target: node,
                target_addr: addr,
                phase: ProbePhase::Direct,
                deadline: now + self.config.probe_timeout,
            });
        }
    }

    /// Disseminate to up to `gossip_fanout` random live peers plus all seeds, so
    /// the anti-entropy payload (and the replies it draws) converges the cluster.
    async fn gossip_round(&mut self) {
        let mut dests: Vec<SocketAddr> = {
            let shared = self.lock();
            shared
                .peers
                .values()
                .filter(|p| p.state != MemberState::Dead)
                .map(|p| p.addr)
                .collect()
        };
        self.rng.shuffle(&mut dests);
        dests.truncate(self.config.gossip_fanout);
        for seed in &self.config.seeds {
            if !dests.contains(seed) {
                dests.push(*seed);
            }
        }
        for addr in dests {
            let seq = self.next_seq();
            self.send(addr, seq, Kind::Ping).await;
        }
    }

    /// Choose up to `indirect_probes` random live helper peers other than the
    /// probe target.
    fn pick_indirect(&mut self, target: SocketAddr) -> Vec<SocketAddr> {
        let mut helpers: Vec<SocketAddr> = {
            let shared = self.lock();
            shared
                .peers
                .values()
                .filter(|p| p.state == MemberState::Alive && p.addr != target)
                .map(|p| p.addr)
                .collect()
        };
        self.rng.shuffle(&mut helpers);
        helpers.truncate(self.config.indirect_probes);
        helpers
    }

    /// Mark a peer `Suspect` (only from `Alive`) and arm its suspicion deadline.
    fn mark_suspect(&self, node: &NodeId, now: Instant) {
        let mut shared = self.lock();
        let timeout = self.config.suspicion_timeout;
        if let Some(peer) = shared.peers.get_mut(node) {
            if peer.state == MemberState::Alive {
                peer.state = MemberState::Suspect;
                peer.suspect_deadline = Some(now + timeout);
            }
        }
    }

    /// Mark a peer `Alive` (clearing any suspicion) and ensure it is in the
    /// live membership view.
    fn mark_alive(&self, node: &NodeId) {
        let mut shared = self.lock();
        if let Some(peer) = shared.peers.get_mut(node) {
            if peer.state != MemberState::Dead {
                peer.state = MemberState::Alive;
                peer.suspect_deadline = None;
                shared.membership.join(node.clone());
            }
        }
    }

    /// Decode, validate, reconcile, and act on an incoming datagram.
    async fn handle(&mut self, bytes: &[u8]) {
        let Ok(frame) = Frame::decode(bytes) else {
            return;
        };
        if frame.version != PROTOCOL_VERSION {
            return;
        }

        let refuted = {
            let mut shared = self.lock();
            self.reconcile(&mut shared, &frame)
        };
        if refuted {
            // Self was suspected/declared dead; we bumped our incarnation, so
            // re-broadcast the corrected state immediately.
            self.gossip_round().await;
        }

        match frame.kind {
            Kind::Ping => {
                self.send(frame.from_addr, frame.seq, Kind::Ack).await;
            }
            Kind::PingReq { target } => {
                self.relays.insert(
                    frame.seq,
                    Relay {
                        origin: frame.from_addr,
                        target,
                        expire: Instant::now() + self.config.probe_timeout * 2,
                    },
                );
                self.send(target, frame.seq, Kind::Ping).await;
            }
            Kind::Ack => self.handle_ack(&frame).await,
        }
    }

    /// Process an ack: complete our own probe if the sequence matches, and/or
    /// forward it to an indirect-probe origin if we are relaying for them.
    async fn handle_ack(&mut self, frame: &Frame) {
        let mut completed = None;
        if let Some(probe) = &self.probe {
            if probe.seq == frame.seq {
                completed = Some(probe.target.clone());
            }
        }
        if let Some(node) = completed {
            self.probe = None;
            self.mark_alive(&node);
        }

        if let Some(relay) = self.relays.remove(&frame.seq) {
            if relay.target == frame.from_addr {
                self.send(relay.origin, frame.seq, Kind::Ack).await;
            }
        }
    }

    /// Fold a frame's anti-entropy payload into shared state. Returns `true` if
    /// the frame suspected/killed *this* node and we refuted by bumping our
    /// incarnation.
    fn reconcile(&self, shared: &mut Shared, frame: &Frame) -> bool {
        let mut refuted = false;
        for update in &frame.updates {
            refuted |= self.apply_update(shared, update);
        }
        shared.counter.merge(&frame.counter);
        refuted
    }

    /// Apply one [`MemberUpdate`] using `(incarnation, state-rank)` precedence.
    fn apply_update(&self, shared: &mut Shared, update: &MemberUpdate) -> bool {
        if update.node == shared.me {
            if matches!(update.state, MemberState::Suspect | MemberState::Dead)
                && update.incarnation >= shared.incarnation
            {
                shared.incarnation = update.incarnation + 1;
                return true;
            }
            return false;
        }

        let now = Instant::now();
        let timeout = self.config.suspicion_timeout;
        match shared.peers.get_mut(&update.node) {
            None => {
                let suspect_deadline = match update.state {
                    MemberState::Suspect => Some(now + timeout),
                    _ => None,
                };
                shared.peers.insert(
                    update.node.clone(),
                    Peer {
                        addr: update.addr,
                        incarnation: update.incarnation,
                        state: update.state,
                        suspect_deadline,
                    },
                );
                if update.state != MemberState::Dead {
                    shared.membership.join(update.node.clone());
                }
            }
            Some(peer) => {
                let supersedes = update.incarnation > peer.incarnation
                    || (update.incarnation == peer.incarnation
                        && update.state.rank() > peer.state.rank());
                if supersedes {
                    peer.incarnation = update.incarnation;
                    peer.addr = update.addr;
                    peer.state = update.state;
                    match update.state {
                        MemberState::Alive => {
                            peer.suspect_deadline = None;
                            shared.membership.join(update.node.clone());
                        }
                        MemberState::Suspect => {
                            peer.suspect_deadline = Some(now + timeout);
                            shared.membership.join(update.node.clone());
                        }
                        MemberState::Dead => {
                            peer.suspect_deadline = None;
                            shared.membership.leave(&update.node);
                        }
                    }
                }
            }
        }
        false
    }
}

/// A handle to a running cluster node.
///
/// [`Cluster::spawn`] binds the UDP socket, resolves the local address (which
/// becomes the node's identity), and launches the background gossip runtime. The
/// node participates in failure detection and anti-entropy until
/// [`Cluster::shutdown`] is awaited or the handle is dropped.
pub struct Cluster {
    me: NodeId,
    local_addr: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    shutdown: Arc<Notify>,
    handle: Option<JoinHandle<()>>,
}

impl Cluster {
    /// Bind, resolve the local address, and spawn the gossip runtime.
    ///
    /// The resolved [`local_addr`](Cluster::local_addr) — the OS-assigned
    /// address of the bound socket — is this node's stable identity and the
    /// value seed peers should target.
    ///
    /// # Errors
    ///
    /// Returns any [`io::Error`] from binding the UDP socket or reading its
    /// local address.
    pub async fn spawn(config: Config) -> io::Result<Self> {
        let socket = UdpSocket::bind(config.bind).await?;
        let local_addr = socket.local_addr()?;
        let me = NodeId::new(local_addr.to_string());
        let shared = Arc::new(Mutex::new(Shared::new(me.clone(), local_addr)));
        let shutdown = Arc::new(Notify::new());
        let runtime = Runtime::new(socket, config, shared.clone());
        let handle = tokio::spawn(runtime.run(shutdown.clone()));
        Ok(Self {
            me,
            local_addr,
            shared,
            shutdown,
            handle: Some(handle),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Increment this node's slot of the shared grow-only counter by `n`. The
    /// increment propagates to the rest of the cluster via anti-entropy.
    pub fn incr(&self, n: u64) {
        let mut shared = self.lock();
        let me = shared.me.clone();
        shared.counter.incr(&me, n);
    }

    /// The merged value of the shared counter across all known nodes.
    #[must_use]
    pub fn counter_value(&self) -> u64 {
        self.lock().counter.value()
    }

    /// The current live membership view, in sorted order.
    #[must_use]
    pub fn members(&self) -> Vec<NodeId> {
        self.lock()
            .membership
            .members()
            .into_iter()
            .cloned()
            .collect()
    }

    /// The deterministically-elected leader (lowest node id), if any.
    #[must_use]
    pub fn leader(&self) -> Option<NodeId> {
        self.lock().membership.leader().cloned()
    }

    /// Whether this node is currently the elected leader.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        self.lock().membership.is_leader()
    }

    /// This node's identifier.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.me.clone()
    }

    /// This node's resolved local UDP address (its identity).
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Signal the runtime to stop and await its task, releasing the socket.
    pub async fn shutdown(mut self) {
        self.shutdown.notify_one();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
