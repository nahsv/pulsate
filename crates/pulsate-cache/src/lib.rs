//! `pulsate-cache` — RFC-9111 HTTP caching (`docs/08-cache.md`).
//!
//! Cache keys are method + host + path + `Vary` header values. Cacheability is
//! decided by method, status, `Cache-Control`, and `Set-Cookie`; freshness comes
//! from `s-maxage`/`max-age` or a configured default, with stale-while-revalidate
//! and tag-based purge. Only an in-memory store is supported.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, StatusCode};

/// Per-cache policy (`cache <name> { ... }`).
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// TTL when the response does not specify its own freshness.
    pub default_ttl: Duration,
    /// Methods eligible for caching (typically `GET`, `HEAD`).
    pub methods: Vec<String>,
    /// Request header names that compose the cache key (the `Vary` dimension).
    pub vary: Vec<String>,
    /// How long a stale entry may still be served while revalidating.
    pub stale_while_revalidate: Duration,
    /// Maximum number of stored entries (coarse memory bound).
    pub max_entries: usize,
    /// Largest single response body that may be cached, in bytes. Bodies above
    /// this are never stored (M13).
    pub max_body_bytes: usize,
    /// Total byte budget across all stored bodies. Inserts that would exceed it
    /// evict the oldest entries first (size-aware LRU; M13).
    pub max_total_bytes: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            default_ttl: Duration::from_secs(60),
            methods: vec!["GET".into(), "HEAD".into()],
            vary: Vec::new(),
            stale_while_revalidate: Duration::ZERO,
            max_entries: 10_000,
            // 1 MiB per body, 256 MiB total: bounded memory regardless of count.
            max_body_bytes: 1 << 20,
            max_total_bytes: 256 << 20,
        }
    }
}

/// Freshness state of a cache hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Within its freshness lifetime.
    Fresh,
    /// Past freshness but within the stale-while-revalidate window.
    Stale,
}

/// A served cache hit: the stored response plus its computed age.
#[derive(Debug, Clone)]
pub struct Hit {
    /// Stored status code.
    pub status: StatusCode,
    /// Stored response headers (name, value), lowercase names.
    pub headers: Vec<(String, String)>,
    /// Stored body.
    pub body: Bytes,
    /// Age in seconds (for the `Age` header).
    pub age_secs: u64,
    /// Whether the entry is fresh or being served stale.
    pub freshness: Freshness,
}

#[derive(Debug, Clone)]
struct Entry {
    status: StatusCode,
    headers: Vec<(String, String)>,
    body: Bytes,
    stored_at: Instant,
    ttl: Duration,
    swr: Duration,
    tags: Vec<String>,
}

/// The store's inner state: the entry map plus a running total of cached body
/// bytes, both guarded by one mutex so they never drift.
#[derive(Debug, Default)]
struct Inner {
    map: HashMap<String, Entry>,
    /// Sum of `body.len()` across every entry in `map`.
    bytes: usize,
}

impl Inner {
    /// Remove one entry and decrement the byte total to match.
    fn remove(&mut self, key: &str) -> Option<Entry> {
        let entry = self.map.remove(key)?;
        self.bytes = self.bytes.saturating_sub(entry.body.len());
        Some(entry)
    }
}

/// An in-memory cache store, shared behind an `Arc`. Reads and writes take one
/// mutex; eviction enforces both a count cap and a total-byte budget with
/// size-aware (oldest-first) eviction.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |i| i.map.len())
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total bytes currently held across all cached bodies.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.inner.lock().map_or(0, |i| i.bytes)
    }

    /// Purge every entry carrying `tag`. Returns the number removed.
    pub fn purge_tag(&self, tag: &str) -> usize {
        let Ok(mut inner) = self.inner.lock() else {
            return 0;
        };
        let before = inner.map.len();
        let mut freed = 0;
        inner.map.retain(|_, e| {
            let keep = !e.tags.iter().any(|t| t == tag);
            if !keep {
                freed += e.body.len();
            }
            keep
        });
        inner.bytes = inner.bytes.saturating_sub(freed);
        before - inner.map.len()
    }

    /// Remove all entries.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.map.clear();
            inner.bytes = 0;
        }
    }
}

/// A cache bound to a route: a shared store plus this cache's policy.
#[derive(Debug, Clone)]
pub struct CacheLayer {
    store: std::sync::Arc<MemoryStore>,
    config: CacheConfig,
}

impl CacheLayer {
    /// Bind a store and config together.
    #[must_use]
    pub fn new(store: std::sync::Arc<MemoryStore>, config: CacheConfig) -> Self {
        Self { store, config }
    }

    /// The shared store.
    #[must_use]
    pub fn store(&self) -> &std::sync::Arc<MemoryStore> {
        &self.store
    }

    /// Compute the cache key for a request.
    ///
    /// `target` is the full request target (path **and** raw query string), so
    /// `/p?a` and `/p?b` never collide into one entry (cache poisoning, H4).
    #[must_use]
    pub fn key(&self, method: &str, host: &str, target: &str, req_headers: &HeaderMap) -> String {
        let mut key = format!("{method}\n{host}\n{target}");
        for name in &self.config.vary {
            let v = req_headers
                .get(name.as_str())
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            key.push('\n');
            key.push_str(name);
            key.push('=');
            key.push_str(v);
        }
        key
    }

    /// Whether the request permits a cached response (no `Cache-Control: no-store`).
    #[must_use]
    pub fn request_allows_cache(&self, method: &str, req_headers: &HeaderMap) -> bool {
        if !self
            .config
            .methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method))
        {
            return false;
        }
        !cache_control(req_headers).contains_key("no-store")
    }

    /// Look up a key, returning a hit if a fresh or stale-servable entry exists.
    /// Fully-expired entries are evicted and reported as a miss.
    #[must_use]
    pub fn lookup(&self, key: &str) -> Option<Hit> {
        let mut inner = self.store.inner.lock().ok()?;
        let age = inner.map.get(key)?.stored_at.elapsed();
        let (ttl, swr) = {
            let e = inner.map.get(key)?;
            (e.ttl, e.swr)
        };
        let freshness = if age <= ttl {
            Freshness::Fresh
        } else if age <= ttl + swr {
            Freshness::Stale
        } else {
            inner.remove(key);
            return None;
        };
        let entry = inner.map.get(key)?;
        Some(Hit {
            status: entry.status,
            headers: entry.headers.clone(),
            body: entry.body.clone(),
            age_secs: age.as_secs(),
            freshness,
        })
    }

    /// Store a response if it is cacheable. Returns `true` if stored.
    pub fn maybe_store(
        &self,
        key: &str,
        status: StatusCode,
        resp_headers: &HeaderMap,
        body: &Bytes,
    ) -> bool {
        let Some(ttl) = self.cacheable_ttl(status, resp_headers) else {
            return false;
        };
        // Refuse oversized bodies outright (M13).
        if body.len() > self.config.max_body_bytes {
            return false;
        }
        let headers: Vec<(String, String)> = resp_headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_string(), s.to_string()))
            })
            .collect();
        let tags = parse_tags(resp_headers);

        let Ok(mut inner) = self.store.inner.lock() else {
            return false;
        };
        // Replacing an existing key frees its old bytes first.
        inner.remove(key);
        if inner.map.len() >= self.config.max_entries {
            return false; // at capacity; refuse new keys
        }
        // Evict oldest entries until this body fits the total-byte budget (M13).
        evict_until_fits(&mut inner, body.len(), self.config.max_total_bytes);
        if inner.bytes + body.len() > self.config.max_total_bytes {
            return false; // single body larger than the whole budget
        }
        inner.bytes += body.len();
        inner.map.insert(
            key.to_string(),
            Entry {
                status,
                headers,
                body: body.clone(),
                stored_at: Instant::now(),
                ttl,
                swr: self.config.stale_while_revalidate,
                tags,
            },
        );
        true
    }

    /// Determine the freshness lifetime for a response, or `None` if it must not
    /// be cached.
    fn cacheable_ttl(&self, status: StatusCode, headers: &HeaderMap) -> Option<Duration> {
        if status != StatusCode::OK {
            return None;
        }
        if headers.contains_key(http::header::SET_COOKIE) {
            return None;
        }
        // Respect the response `Vary` header. We only key on the configured
        // `vary` dimensions, so a response that varies on anything else (e.g.
        // `Authorization`) cannot be safely reused across requests (H5).
        if let Some(vary) = headers
            .get(http::header::VARY)
            .and_then(|v| v.to_str().ok())
        {
            for field in vary.split(',') {
                let field = field.trim();
                if field.is_empty() {
                    continue;
                }
                if field == "*"
                    || !self
                        .config
                        .vary
                        .iter()
                        .any(|v| v.eq_ignore_ascii_case(field))
                {
                    return None;
                }
            }
        }
        let cc = cache_control(headers);
        if cc.contains_key("no-store") || cc.contains_key("private") {
            return None;
        }
        // `no-cache` means store but revalidate before use → ttl 0.
        if cc.contains_key("no-cache") {
            return Some(Duration::ZERO);
        }
        if let Some(secs) = cc.get("s-maxage").or_else(|| cc.get("max-age")) {
            if let Ok(secs) = secs.parse::<u64>() {
                return Some(Duration::from_secs(secs));
            }
        }
        Some(self.config.default_ttl)
    }
}

/// Evict the oldest entries until `incoming` more bytes fit within `budget`
/// (size-aware LRU by store time). Stops if the map empties (M13).
fn evict_until_fits(inner: &mut Inner, incoming: usize, budget: usize) {
    while inner.bytes + incoming > budget {
        let Some(oldest) = inner
            .map
            .iter()
            .min_by_key(|(_, e)| e.stored_at)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        if inner.remove(&oldest).is_none() {
            break;
        }
    }
}

/// Parse a `Cache-Control` header into a directive map (lowercased keys).
fn cache_control(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(v) = headers
        .get(http::header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
    {
        for part in v.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (k, val) = part.split_once('=').unwrap_or((part, ""));
            out.insert(
                k.trim().to_ascii_lowercase(),
                val.trim().trim_matches('"').to_string(),
            );
        }
    }
    out
}

/// Parse cache tags from a `Cache-Tag` / `X-Cache-Tag` header (comma-separated).
fn parse_tags(headers: &HeaderMap) -> Vec<String> {
    headers
        .get("cache-tag")
        .or_else(|| headers.get("x-cache-tag"))
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn layer() -> CacheLayer {
        CacheLayer::new(Arc::new(MemoryStore::new()), CacheConfig::default())
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                http::HeaderName::try_from(*k).unwrap(),
                http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn stores_and_serves_a_hit() {
        let l = layer();
        let key = l.key("GET", "h", "/p", &HeaderMap::new());
        assert!(l.lookup(&key).is_none());
        assert!(l.maybe_store(
            &key,
            StatusCode::OK,
            &HeaderMap::new(),
            &Bytes::from_static(b"hi")
        ));
        let hit = l.lookup(&key).expect("hit");
        assert_eq!(hit.body, Bytes::from_static(b"hi"));
        assert_eq!(hit.freshness, Freshness::Fresh);
    }

    #[test]
    fn no_store_and_set_cookie_are_not_cached() {
        let l = layer();
        let key = l.key("GET", "h", "/p", &HeaderMap::new());
        assert!(!l.maybe_store(
            &key,
            StatusCode::OK,
            &headers(&[("cache-control", "no-store")]),
            &Bytes::new()
        ));
        assert!(!l.maybe_store(
            &key,
            StatusCode::OK,
            &headers(&[("set-cookie", "x=1")]),
            &Bytes::new()
        ));
        assert!(!l.maybe_store(
            &key,
            StatusCode::NOT_FOUND,
            &HeaderMap::new(),
            &Bytes::new()
        ));
    }

    #[test]
    fn request_no_store_bypasses_cache() {
        let l = layer();
        assert!(l.request_allows_cache("GET", &HeaderMap::new()));
        assert!(!l.request_allows_cache("GET", &headers(&[("cache-control", "no-store")])));
        assert!(!l.request_allows_cache("POST", &HeaderMap::new()));
    }

    #[test]
    fn vary_changes_the_key() {
        let cfg = CacheConfig {
            vary: vec!["accept-encoding".into()],
            ..CacheConfig::default()
        };
        let l = CacheLayer::new(Arc::new(MemoryStore::new()), cfg);
        let k1 = l.key("GET", "h", "/p", &headers(&[("accept-encoding", "gzip")]));
        let k2 = l.key("GET", "h", "/p", &headers(&[("accept-encoding", "br")]));
        assert_ne!(k1, k2);
    }

    #[test]
    fn purge_by_tag_removes_tagged_entries() {
        let l = layer();
        let key = l.key("GET", "h", "/p", &HeaderMap::new());
        l.maybe_store(
            &key,
            StatusCode::OK,
            &headers(&[("cache-tag", "products, home")]),
            &Bytes::from_static(b"x"),
        );
        assert_eq!(l.store().len(), 1);
        assert_eq!(l.store().purge_tag("products"), 1);
        assert!(l.store().is_empty());
    }

    #[test]
    fn max_age_overrides_default_ttl() {
        let l = layer();
        let ttl = l.cacheable_ttl(StatusCode::OK, &headers(&[("cache-control", "max-age=5")]));
        assert_eq!(ttl, Some(Duration::from_secs(5)));
    }

    #[test]
    fn distinct_query_strings_do_not_collide() {
        // Two requests differing only in the query string must key separately,
        // otherwise the first response poisons the second (H4).
        let l = layer();
        let k1 = l.key("GET", "h", "/p?attacker", &HeaderMap::new());
        let k2 = l.key("GET", "h", "/p?victim", &HeaderMap::new());
        assert_ne!(k1, k2);
        l.maybe_store(
            &k1,
            StatusCode::OK,
            &HeaderMap::new(),
            &Bytes::from_static(b"a"),
        );
        // The other query is still a miss.
        assert!(l.lookup(&k2).is_none());
        assert_eq!(l.lookup(&k1).unwrap().body, Bytes::from_static(b"a"));
    }

    #[test]
    fn response_vary_on_unkeyed_header_is_not_cached() {
        // The cache keys nothing extra, so a response that varies on
        // `Authorization` must not be stored (H5).
        let l = layer();
        let key = l.key("GET", "h", "/p", &HeaderMap::new());
        assert!(!l.maybe_store(
            &key,
            StatusCode::OK,
            &headers(&[("vary", "Authorization")]),
            &Bytes::from_static(b"secret")
        ));
        assert!(!l.maybe_store(
            &key,
            StatusCode::OK,
            &headers(&[("vary", "*")]),
            &Bytes::from_static(b"x")
        ));
        // A `Vary` naming only a configured dimension is fine.
        let cfg = CacheConfig {
            vary: vec!["accept-encoding".into()],
            ..CacheConfig::default()
        };
        let l2 = CacheLayer::new(Arc::new(MemoryStore::new()), cfg);
        let k = l2.key("GET", "h", "/p", &HeaderMap::new());
        assert!(l2.maybe_store(
            &k,
            StatusCode::OK,
            &headers(&[("vary", "Accept-Encoding")]),
            &Bytes::from_static(b"ok")
        ));
    }

    #[test]
    fn oversized_body_is_refused() {
        let cfg = CacheConfig {
            max_body_bytes: 8,
            ..CacheConfig::default()
        };
        let l = CacheLayer::new(Arc::new(MemoryStore::new()), cfg);
        let key = l.key("GET", "h", "/p", &HeaderMap::new());
        assert!(!l.maybe_store(
            &key,
            StatusCode::OK,
            &HeaderMap::new(),
            &Bytes::from_static(b"too-large-body")
        ));
        assert!(l.store().is_empty());
    }

    #[test]
    fn total_byte_budget_evicts_oldest() {
        let cfg = CacheConfig {
            max_body_bytes: 100,
            max_total_bytes: 10,
            ..CacheConfig::default()
        };
        let l = CacheLayer::new(Arc::new(MemoryStore::new()), cfg);
        let k1 = l.key("GET", "h", "/a", &HeaderMap::new());
        let k2 = l.key("GET", "h", "/b", &HeaderMap::new());
        assert!(l.maybe_store(
            &k1,
            StatusCode::OK,
            &HeaderMap::new(),
            &Bytes::from_static(b"aaaaaa")
        )); // 6 bytes
        assert!(l.maybe_store(
            &k2,
            StatusCode::OK,
            &HeaderMap::new(),
            &Bytes::from_static(b"bbbbbb")
        )); // 6 bytes → 12 > 10, evict /a
        assert!(l.lookup(&k1).is_none(), "oldest evicted");
        assert!(l.lookup(&k2).is_some());
        assert!(l.store().bytes() <= 10);
    }
}
