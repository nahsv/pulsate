//! `pulsate-waf` — web-application-firewall controls (`docs/09-security.md`).
//!
//! A fixed-window [`RateLimiter`], a signature-based [`WafEngine`] (block or
//! detect mode) covering common SQLi / XSS / traversal patterns, an [`IpAcl`]
//! with CIDR allow/deny, and a tamper-evident hash-chained [`AuditLog`].
//! Rate limiting is node-local. Geo, ASN, and bot controls need a MaxMind
//! database and are not implemented.
#![forbid(unsafe_code)]

mod cidr;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pulsate_core::Code;

pub use cidr::Cidr;

/// Why a request was blocked.
#[derive(Debug, Clone)]
pub struct Block {
    /// The stable error code.
    pub code: Code,
    /// The HTTP status to return.
    pub status: u16,
    /// A short reason for logs/audit (never leaked verbatim to clients).
    pub reason: String,
}

impl Block {
    fn new(code: Code, reason: impl Into<String>) -> Self {
        Self {
            code,
            status: code.http_status(),
            reason: reason.into(),
        }
    }
}

/// The outcome of a rate-limit check, carrying the `RateLimit-*` header values.
#[derive(Debug, Clone, Copy)]
pub struct RateOutcome {
    /// Whether the request is within the limit.
    pub allowed: bool,
    /// The configured limit per window.
    pub limit: u64,
    /// Remaining requests in the current window.
    pub remaining: u64,
    /// Seconds until the window resets.
    pub reset_secs: u64,
}

/// A fixed-window rate limiter keyed by an arbitrary string (e.g. client IP).
#[derive(Debug)]
pub struct RateLimiter {
    limit: u64,
    window: Duration,
    buckets: Mutex<HashMap<String, (Instant, u64)>>,
}

impl RateLimiter {
    /// Allow `limit` requests per `window`.
    #[must_use]
    pub fn new(limit: u64, window: Duration) -> Self {
        Self {
            limit,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Count one request against `key` and report whether it is allowed.
    pub fn check(&self, key: &str) -> RateOutcome {
        let now = Instant::now();
        let Ok(mut buckets) = self.buckets.lock() else {
            return RateOutcome {
                allowed: true,
                limit: self.limit,
                remaining: self.limit,
                reset_secs: 0,
            };
        };
        let entry = buckets.entry(key.to_string()).or_insert((now, 0));
        // Reset the window if it has elapsed.
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        let used = entry.1;
        let reset_secs = self
            .window
            .saturating_sub(now.duration_since(entry.0))
            .as_secs();
        RateOutcome {
            allowed: used <= self.limit,
            limit: self.limit,
            remaining: self.limit.saturating_sub(used),
            reset_secs,
        }
    }
}

/// WAF enforcement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Block matching requests.
    Block,
    /// Log matches but allow the request (rollout mode).
    Detect,
}

/// A signature-based WAF engine. Inspects the request path + query string for
/// known-malicious patterns.
#[derive(Debug)]
pub struct WafEngine {
    mode: Mode,
}

/// One built-in signature: an id and a lowercase needle.
struct Signature {
    id: &'static str,
    needle: &'static str,
}

/// The built-in signature set (CRS-lite). Matched case-insensitively against the
/// decoded path + query.
const SIGNATURES: &[Signature] = &[
    Signature {
        id: "sqli-union",
        needle: "union select",
    },
    Signature {
        id: "sqli-or",
        needle: "' or '1'='1",
    },
    Signature {
        id: "sqli-comment",
        needle: "-- ",
    },
    Signature {
        id: "sqli-drop",
        needle: "; drop table",
    },
    Signature {
        id: "xss-script",
        needle: "<script",
    },
    Signature {
        id: "xss-js",
        needle: "javascript:",
    },
    Signature {
        id: "xss-onerror",
        needle: "onerror=",
    },
    Signature {
        id: "traversal",
        needle: "../",
    },
    Signature {
        id: "traversal-enc",
        needle: "..%2f",
    },
    Signature {
        id: "rce-etc-passwd",
        needle: "/etc/passwd",
    },
];

impl WafEngine {
    /// Build an engine in the given mode.
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        Self { mode }
    }

    /// Inspect a request target. Returns `Some(Block)` only when a signature
    /// matches *and* the engine is in [`Mode::Block`].
    #[must_use]
    pub fn inspect(&self, path_and_query: &str) -> Option<Block> {
        let matched = self.detect(path_and_query)?;
        if self.mode == Mode::Block {
            Some(Block::new(
                Code::WAF_RULE,
                format!("matched signature {matched}"),
            ))
        } else {
            None // detect mode: caller may still audit the match via `detect`
        }
    }

    /// Report the signature id that matched, regardless of mode (for audit/detect).
    #[must_use]
    pub fn detect(&self, path_and_query: &str) -> Option<&'static str> {
        let haystack = path_and_query.to_ascii_lowercase();
        SIGNATURES
            .iter()
            .find(|s| haystack.contains(s.needle))
            .map(|s| s.id)
    }
}

/// An IP allow/deny list. `allow` entries take precedence over `deny`, so a
/// trusted range can be carved out of a broader block.
#[derive(Debug, Default)]
pub struct IpAcl {
    allow: Vec<Cidr>,
    deny: Vec<Cidr>,
}

impl IpAcl {
    /// An empty ACL (allows everything).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an allow rule.
    #[must_use]
    pub fn allow(mut self, cidr: Cidr) -> Self {
        self.allow.push(cidr);
        self
    }

    /// Add a deny rule.
    #[must_use]
    pub fn deny(mut self, cidr: Cidr) -> Self {
        self.deny.push(cidr);
        self
    }

    /// Decide whether `ip` is blocked.
    #[must_use]
    pub fn check(&self, ip: IpAddr) -> Option<Block> {
        if self.allow.iter().any(|c| c.contains(ip)) {
            return None;
        }
        if self.deny.iter().any(|c| c.contains(ip)) {
            return Some(Block::new(Code::WAF_IP_DENIED, format!("ip {ip} denied")));
        }
        None
    }
}

/// One entry in the tamper-evident audit chain.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Monotonic sequence number.
    pub seq: u64,
    /// The event description.
    pub event: String,
    /// Hash of the previous entry (chains the log).
    pub prev_hash: u64,
    /// Hash of this entry.
    pub hash: u64,
}

/// A hash-chained audit log: each entry's hash covers the previous hash, so any
/// retroactive edit breaks the chain.
#[derive(Debug, Default)]
pub struct AuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

impl AuditLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an event, returning its sequence number.
    pub fn append(&self, event: impl Into<String>) -> u64 {
        let event = event.into();
        let Ok(mut entries) = self.entries.lock() else {
            return 0;
        };
        let (seq, prev_hash) = entries.last().map_or((0, 0), |e| (e.seq + 1, e.hash));
        let hash = chain_hash(prev_hash, seq, &event);
        entries.push(AuditEntry {
            seq,
            event,
            prev_hash,
            hash,
        });
        seq
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().map_or(0, |e| e.len())
    }

    /// Whether the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Verify the hash chain end-to-end.
    #[must_use]
    pub fn verify(&self) -> bool {
        let Ok(entries) = self.entries.lock() else {
            return false;
        };
        let mut prev = 0;
        for e in entries.iter() {
            if e.prev_hash != prev || e.hash != chain_hash(prev, e.seq, &e.event) {
                return false;
            }
            prev = e.hash;
        }
        true
    }

    /// Expose a copy of the entries (for tests / the admin API).
    #[must_use]
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries
            .lock()
            .map_or_else(|_| Vec::new(), |e| e.clone())
    }
}

fn chain_hash(prev: u64, seq: u64, event: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ prev;
    for byte in seq.to_le_bytes().iter().chain(event.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_blocks_over_limit() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        assert!(rl.check("ip").allowed);
        assert!(rl.check("ip").allowed);
        let third = rl.check("ip");
        assert!(!third.allowed);
        assert_eq!(third.remaining, 0);
        // A different key has its own budget.
        assert!(rl.check("other").allowed);
    }

    #[test]
    fn waf_blocks_in_block_mode_only() {
        let block = WafEngine::new(Mode::Block);
        assert!(block.inspect("/search?q=1 UNION SELECT password").is_some());
        assert!(block.inspect("/normal/path").is_none());

        let detect = WafEngine::new(Mode::Detect);
        assert!(detect.inspect("/x?q=<script>").is_none()); // detect never blocks
        assert_eq!(detect.detect("/x?q=<script>"), Some("xss-script"));
    }

    #[test]
    fn ip_acl_denies_with_allow_override() {
        let acl = IpAcl::new()
            .deny("10.0.0.0/8".parse().unwrap())
            .allow("10.1.2.3/32".parse().unwrap());
        assert!(acl.check("10.9.9.9".parse().unwrap()).is_some()); // denied
        assert!(acl.check("10.1.2.3".parse().unwrap()).is_none()); // carved out
        assert!(acl.check("192.0.2.1".parse().unwrap()).is_none()); // not in deny
    }

    #[test]
    fn audit_log_is_tamper_evident() {
        let log = AuditLog::new();
        log.append("blocked ip 10.0.0.1");
        log.append("waf sqli /search");
        assert_eq!(log.len(), 2);
        assert!(log.verify());

        // Recomputing with a different event for the same seq must not match the
        // stored hash — proving the chain detects edits.
        let entries = log.entries();
        let forged = chain_hash(entries[0].hash, entries[1].seq, "tampered");
        assert_ne!(forged, entries[1].hash);
    }
}
