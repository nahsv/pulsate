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

/// Maximum percent-decode passes when normalizing a target. Bounds work while
/// reaching a fixed point for multiply-encoded payloads (M9).
const MAX_DECODE_PASSES: usize = 8;

/// A signature-based WAF engine. Inspects the request path + query string for
/// known-malicious patterns.
///
/// Scope: this engine inspects only the request **target** (path + query). It
/// does not see request/response bodies or headers — those are out of scope for
/// the signature set (M9).
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
        let haystack = normalize(path_and_query);
        SIGNATURES
            .iter()
            .find(|s| haystack.contains(s.needle))
            .map(|s| s.id)
    }
}

/// Normalize a request target before signature matching so common evasions
/// (multi-encoding, inline comments, whitespace tricks) cannot slip past (M9):
///
/// 1. percent-decode repeatedly until the string stops changing (fixed point);
/// 2. lowercase;
/// 3. strip `/* ... */` comment runs (e.g. `union/**/select`);
/// 4. collapse every whitespace run to a single space.
fn normalize(input: &str) -> String {
    let mut current = input.to_string();
    for _ in 0..MAX_DECODE_PASSES {
        let decoded = percent_decode(&current);
        if decoded == current {
            break;
        }
        current = decoded;
    }
    let lowered = current.to_ascii_lowercase();
    let no_comments = strip_block_comments(&lowered);
    collapse_whitespace(&no_comments)
}

/// Percent-decode `s` (lossy on invalid UTF-8); leaves stray `%` untouched.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replace `/* ... */` comment runs with a single space.
fn strip_block_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            // Skip until the closing `*/` (or end of input).
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            out.push(' ');
        } else {
            out.push(char::from(bytes[i]));
            i += 1;
        }
    }
    out
}

/// Collapse every run of ASCII whitespace into a single space.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
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
    ///
    /// The IP is first canonicalized (IPv4-mapped IPv6 like `::ffff:1.2.3.4` →
    /// `1.2.3.4`) so a mapped address cannot slip past a v4 rule (M7). When any
    /// allow rule exists, an address matching none of them is denied by default
    /// rather than allowed (fail-closed, M8).
    #[must_use]
    pub fn check(&self, ip: IpAddr) -> Option<Block> {
        let ip = ip.to_canonical();
        // Allow rules take precedence: an explicit allow short-circuits a deny.
        if self.allow.iter().any(|c| c.contains(ip)) {
            return None;
        }
        // Allow-list present but unmatched → default deny (M8).
        if !self.allow.is_empty() {
            return Some(Block::new(
                Code::WAF_IP_DENIED,
                format!("ip {ip} not in allow list"),
            ));
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
    /// Keyed hash of the previous entry, hex-encoded (chains the log). Empty for
    /// the genesis entry.
    pub prev_hash: String,
    /// Keyed HMAC-SHA256 of this entry, hex-encoded.
    pub hash: String,
}

/// A keyed, hash-chained audit log: each entry's HMAC-SHA256 covers the previous
/// hash, the sequence number, and the event, under a server secret. Without the
/// key an attacker cannot recompute a valid chain, so any retroactive edit is
/// detectable (M10).
pub struct AuditLog {
    entries: Mutex<Vec<AuditEntry>>,
    key: ring::hmac::Key,
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("entries", &self.len())
            .finish_non_exhaustive()
    }
}

impl AuditLog {
    /// An empty log keyed by a fresh random server secret (generated from the OS
    /// CSPRNG at startup).
    #[must_use]
    pub fn new() -> Self {
        use ring::rand::{SecureRandom, SystemRandom};
        let mut secret = [0u8; 32];
        SystemRandom::new()
            .fill(&mut secret)
            .expect("operating-system CSPRNG must be available at startup");
        Self::with_key(&secret)
    }

    /// An empty log keyed by an explicit secret (e.g. a shared, persisted key).
    #[must_use]
    pub fn with_key(secret: &[u8]) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            key: ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret),
        }
    }

    /// Append an event, returning its sequence number.
    pub fn append(&self, event: impl Into<String>) -> u64 {
        let event = event.into();
        let Ok(mut entries) = self.entries.lock() else {
            return 0;
        };
        let (seq, prev_hash) = entries
            .last()
            .map_or((0, String::new()), |e| (e.seq + 1, e.hash.clone()));
        let hash = chain_hash(&self.key, &prev_hash, seq, &event);
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

    /// Verify the keyed hash chain end-to-end.
    #[must_use]
    pub fn verify(&self) -> bool {
        let Ok(entries) = self.entries.lock() else {
            return false;
        };
        let mut prev = String::new();
        for e in entries.iter() {
            if e.prev_hash != prev || e.hash != chain_hash(&self.key, &prev, e.seq, &e.event) {
                return false;
            }
            prev.clone_from(&e.hash);
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

/// Compute the keyed chain hash (HMAC-SHA256) for an entry, hex-encoded.
fn chain_hash(key: &ring::hmac::Key, prev: &str, seq: u64, event: &str) -> String {
    let mut ctx = ring::hmac::Context::with_key(key);
    ctx.update(prev.as_bytes());
    ctx.update(&seq.to_le_bytes());
    ctx.update(event.as_bytes());
    hex(ctx.sign().as_ref())
}

/// Lower-case hex-encode bytes.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
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
    fn ip_acl_deny_only_blocks_listed_ranges() {
        // With no allow rules, only denied ranges are blocked.
        let acl = IpAcl::new().deny("10.0.0.0/8".parse().unwrap());
        assert!(acl.check("10.9.9.9".parse().unwrap()).is_some()); // denied
        assert!(acl.check("192.0.2.1".parse().unwrap()).is_none()); // allowed
    }

    #[test]
    fn ip_acl_allow_overrides_deny() {
        let acl = IpAcl::new()
            .deny("10.0.0.0/8".parse().unwrap())
            .allow("10.1.2.3/32".parse().unwrap());
        assert!(acl.check("10.1.2.3".parse().unwrap()).is_none()); // carved out
        assert!(acl.check("10.9.9.9".parse().unwrap()).is_some()); // still denied
    }

    #[test]
    fn allow_list_is_fail_closed() {
        // An allow rule that matches nothing must deny unmatched IPs (M8).
        let acl = IpAcl::new().allow("203.0.113.0/24".parse().unwrap());
        assert!(acl.check("203.0.113.7".parse().unwrap()).is_none()); // allowed
        assert!(acl.check("198.51.100.1".parse().unwrap()).is_some()); // default deny
    }

    #[test]
    fn ipv4_mapped_ipv6_is_canonicalized() {
        // `::ffff:10.0.0.1` must be treated as the v4 address for ACL checks (M7).
        let acl = IpAcl::new().deny("10.0.0.0/8".parse().unwrap());
        let mapped: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
        assert!(
            acl.check(mapped).is_some(),
            "mapped v6 must hit the v4 deny rule"
        );
    }

    #[test]
    fn waf_resists_encoding_and_comment_bypass() {
        let waf = WafEngine::new(Mode::Block);
        // Double-encoded space between UNION and SELECT.
        assert!(
            waf.inspect("/x?q=union%2520select%2520*").is_some(),
            "multi-encoded payload must be caught (M9)"
        );
        // Inline SQL comment used as a whitespace substitute.
        assert!(
            waf.inspect("/x?q=union/**/select").is_some(),
            "comment-obfuscated payload must be caught (M9)"
        );
    }

    #[test]
    fn audit_log_is_tamper_evident() {
        let log = AuditLog::with_key(b"server-secret");
        log.append("blocked ip 10.0.0.1");
        log.append("waf sqli /search");
        assert_eq!(log.len(), 2);
        assert!(log.verify());

        let entries = log.entries();
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"server-secret");
        // Recomputing entry 1 with a forged event under the real key differs —
        // the keyed chain detects edits.
        let forged = chain_hash(&key, &entries[0].hash, entries[1].seq, "tampered");
        assert_ne!(forged, entries[1].hash);

        // An attacker without the key cannot reproduce the legitimate hash.
        let wrong_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"attacker-guess");
        let attacker = chain_hash(
            &wrong_key,
            &entries[0].hash,
            entries[1].seq,
            &entries[1].event,
        );
        assert_ne!(attacker, entries[1].hash);
    }
}
