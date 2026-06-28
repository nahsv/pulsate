//! `pulsate-proxy` — the reverse-proxy core: upstream pools, load balancing,
//! retries, and passive circuit-breaking.
//!
//! An [`Upstream`] is a pool of [`Backend`] targets with a [`Policy`], a
//! [`RetryPolicy`], and a [`BreakerPolicy`]. Per-target health state lives in
//! atomics, keeping the hot path off locks. The [`forward`](forward::forward)
//! entry point picks a healthy target, forwards the request with the correct
//! `X-Forwarded-*` / `Via` headers, retries on connect errors or configured
//! statuses, and ejects a target that fails repeatedly
//! (`docs/06-reverse-proxy.md`).
//!
//! Targets are static and ejection is passive: there are no active health
//! checks and no DNS or Kubernetes discovery.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub mod forward;

#[doc(inline)]
pub use forward::{forward, ProxyClient};

/// Load-balancing policy across an upstream's targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Rotate through targets in order.
    RoundRobin,
    /// Pick the target with the fewest in-flight requests.
    LeastConn,
    /// Pick a pseudo-random target.
    Random,
    /// Pick by a hash of the client IP (sticky by client).
    IpHash,
}

impl Policy {
    /// Parse a policy keyword, defaulting to round-robin for unknown values.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "least_conn" => Policy::LeastConn,
            "random" => Policy::Random,
            "ip_hash" => Policy::IpHash,
            _ => Policy::RoundRobin,
        }
    }
}

/// Retry policy for an upstream.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum additional attempts after the first.
    pub attempts: u32,
    /// Response statuses that trigger a retry.
    pub retry_on_status: Vec<u16>,
    /// Whether a connect error triggers a retry.
    pub on_connect_error: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 1,
            retry_on_status: vec![502, 503, 504],
            on_connect_error: true,
        }
    }
}

/// Passive circuit-breaker / ejection policy.
#[derive(Debug, Clone, Copy)]
pub struct BreakerPolicy {
    /// Consecutive failures before a target is ejected.
    pub consecutive_failures: u32,
    /// How long an ejected target stays out of rotation.
    pub open_for: Duration,
}

impl Default for BreakerPolicy {
    fn default() -> Self {
        Self {
            consecutive_failures: 5,
            open_for: Duration::from_secs(15),
        }
    }
}

/// Per-target runtime health state.
#[derive(Debug, Default)]
struct TargetState {
    inflight: AtomicU64,
    consecutive_failures: AtomicU32,
    ejected_until: Mutex<Option<Instant>>,
}

/// One backend target with its weight and live health state.
#[derive(Debug)]
pub struct Backend {
    url: String,
    #[allow(dead_code)] // read only by weighted policies, which are not implemented
    weight: u32,
    state: TargetState,
}

impl Backend {
    /// The target's base URL (no trailing slash).
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// A named pool of backend targets with balancing and resilience policy.
#[derive(Debug)]
pub struct Upstream {
    /// The upstream name (`@name`).
    pub name: String,
    backends: Vec<Backend>,
    policy: Policy,
    retry: RetryPolicy,
    breaker: BreakerPolicy,
    rr: AtomicUsize,
}

impl Upstream {
    /// Build an upstream pool.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        targets: impl IntoIterator<Item = (String, u32)>,
        policy: Policy,
        retry: RetryPolicy,
        breaker: BreakerPolicy,
    ) -> Self {
        let backends = targets
            .into_iter()
            .map(|(url, weight)| Backend {
                url: url.trim_end_matches('/').to_string(),
                weight: weight.max(1),
                state: TargetState::default(),
            })
            .collect();
        Self {
            name: name.into(),
            backends,
            policy,
            retry,
            breaker,
            rr: AtomicUsize::new(0),
        }
    }

    /// The retry policy.
    #[must_use]
    pub fn retry(&self) -> &RetryPolicy {
        &self.retry
    }

    /// The number of configured targets.
    #[must_use]
    pub fn target_count(&self) -> usize {
        self.backends.len()
    }

    /// The base URL of target `i`.
    #[must_use]
    pub fn target_url(&self, i: usize) -> Option<&str> {
        self.backends.get(i).map(|b| b.url.as_str())
    }

    /// Whether target `i` is currently ejected.
    fn is_ejected(&self, i: usize) -> bool {
        self.backends[i]
            .state
            .ejected_until
            .lock()
            .is_ok_and(|guard| guard.is_some_and(|until| Instant::now() < until))
    }

    /// Pick a healthy target index by the configured policy, or `None` if every
    /// target is currently ejected.
    #[must_use]
    pub fn pick(&self, client_ip: Option<IpAddr>) -> Option<usize> {
        let eligible: Vec<usize> = (0..self.backends.len())
            .filter(|&i| !self.is_ejected(i))
            .collect();
        if eligible.is_empty() {
            return None;
        }
        let chosen = match self.policy {
            Policy::RoundRobin => {
                let n = self.rr.fetch_add(1, Ordering::Relaxed);
                eligible[n % eligible.len()]
            }
            Policy::LeastConn => *eligible
                .iter()
                .min_by_key(|&&i| self.backends[i].state.inflight.load(Ordering::Relaxed))
                .expect("eligible is non-empty"),
            Policy::Random => {
                // Counter-derived pseudo-random index (no rng dependency).
                let n = self.rr.fetch_add(1, Ordering::Relaxed);
                let mixed = n.wrapping_mul(2_654_435_761);
                eligible[mixed % eligible.len()]
            }
            Policy::IpHash => {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                client_ip.hash(&mut hasher);
                let h = usize::try_from(hasher.finish() % (eligible.len() as u64)).unwrap_or(0);
                eligible[h]
            }
        };
        Some(chosen)
    }

    /// Record that a request to target `i` succeeded: reset its failure streak
    /// and clear any ejection.
    pub fn record_success(&self, i: usize) {
        if let Some(b) = self.backends.get(i) {
            b.state.consecutive_failures.store(0, Ordering::Relaxed);
            if let Ok(mut guard) = b.state.ejected_until.lock() {
                *guard = None;
            }
        }
    }

    /// Record that a request to target `i` failed; eject the target once it
    /// crosses the breaker threshold.
    pub fn record_failure(&self, i: usize) {
        let Some(b) = self.backends.get(i) else {
            return;
        };
        let n = b.state.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= self.breaker.consecutive_failures {
            if let Ok(mut guard) = b.state.ejected_until.lock() {
                *guard = Some(Instant::now() + self.breaker.open_for);
            }
            b.state.consecutive_failures.store(0, Ordering::Relaxed);
        }
    }

    fn inflight_inc(&self, i: usize) {
        if let Some(b) = self.backends.get(i) {
            b.state.inflight.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn inflight_dec(&self, i: usize) {
        if let Some(b) = self.backends.get(i) {
            b.state.inflight.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// A registry of named upstream pools, resolved by `@name`.
#[derive(Debug, Default)]
pub struct Registry {
    map: HashMap<String, std::sync::Arc<Upstream>>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an upstream pool.
    pub fn insert(&mut self, upstream: Upstream) {
        self.map
            .insert(upstream.name.clone(), std::sync::Arc::new(upstream));
    }

    /// Resolve an upstream by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<std::sync::Arc<Upstream>> {
        self.map.get(name).cloned()
    }

    /// A `(name, target_count)` summary of every pool, sorted by name (for the
    /// admin API).
    #[must_use]
    pub fn summary(&self) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = self
            .map
            .values()
            .map(|u| (u.name.clone(), u.target_count()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The number of registered upstreams.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(policy: Policy) -> Upstream {
        Upstream::new(
            "api",
            [("http://a:1".to_string(), 1), ("http://b:2".to_string(), 1)],
            policy,
            RetryPolicy::default(),
            BreakerPolicy {
                consecutive_failures: 2,
                open_for: Duration::from_secs(60),
            },
        )
    }

    #[test]
    fn round_robin_rotates() {
        let u = upstream(Policy::RoundRobin);
        let a = u.pick(None).unwrap();
        let b = u.pick(None).unwrap();
        assert_ne!(a, b, "round robin should alternate two targets");
    }

    #[test]
    fn breaker_ejects_after_threshold_then_other_target_is_used() {
        let u = upstream(Policy::RoundRobin);
        // Fail target 0 twice → ejected (threshold 2).
        u.record_failure(0);
        u.record_failure(0);
        assert!(u.is_ejected(0));
        for _ in 0..5 {
            assert_eq!(u.pick(None), Some(1));
        }
    }

    #[test]
    fn success_clears_failure_streak() {
        let u = upstream(Policy::RoundRobin);
        u.record_failure(0);
        u.record_success(0);
        u.record_failure(0); // streak restarted, not yet ejected
        assert!(!u.is_ejected(0));
    }

    #[test]
    fn all_ejected_returns_none() {
        let u = upstream(Policy::RoundRobin);
        u.record_failure(0);
        u.record_failure(0);
        u.record_failure(1);
        u.record_failure(1);
        assert_eq!(u.pick(None), None);
    }

    #[test]
    fn least_conn_prefers_idle_target() {
        let u = upstream(Policy::LeastConn);
        u.inflight_inc(0); // target 0 busy
        assert_eq!(u.pick(None), Some(1));
    }

    #[test]
    fn registry_resolves_by_name() {
        let mut reg = Registry::new();
        reg.insert(upstream(Policy::RoundRobin));
        assert!(reg.get("api").is_some());
        assert!(reg.get("nope").is_none());
        assert_eq!(reg.len(), 1);
    }
}
