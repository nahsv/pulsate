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
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            default_ttl: Duration::from_secs(60),
            methods: vec!["GET".into(), "HEAD".into()],
            vary: Vec::new(),
            stale_while_revalidate: Duration::ZERO,
            max_entries: 10_000,
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

/// An in-memory cache store, shared behind an `Arc`. Reads and writes take one
/// mutex; eviction is a flat count cap, with no LRU or size accounting.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, Entry>>,
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
        self.entries.lock().map_or(0, |m| m.len())
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Purge every entry carrying `tag`. Returns the number removed.
    pub fn purge_tag(&self, tag: &str) -> usize {
        let Ok(mut map) = self.entries.lock() else {
            return 0;
        };
        let before = map.len();
        map.retain(|_, e| !e.tags.iter().any(|t| t == tag));
        before - map.len()
    }

    /// Remove all entries.
    pub fn clear(&self) {
        if let Ok(mut map) = self.entries.lock() {
            map.clear();
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
    #[must_use]
    pub fn key(&self, method: &str, host: &str, path: &str, req_headers: &HeaderMap) -> String {
        let mut key = format!("{method}\n{host}\n{path}");
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
        let mut map = self.store.entries.lock().ok()?;
        let age = map.get(key)?.stored_at.elapsed();
        let (ttl, swr) = {
            let e = map.get(key)?;
            (e.ttl, e.swr)
        };
        let freshness = if age <= ttl {
            Freshness::Fresh
        } else if age <= ttl + swr {
            Freshness::Stale
        } else {
            map.remove(key);
            return None;
        };
        let entry = map.get(key)?;
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
        let headers: Vec<(String, String)> = resp_headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_string(), s.to_string()))
            })
            .collect();
        let tags = parse_tags(resp_headers);

        let Ok(mut map) = self.store.entries.lock() else {
            return false;
        };
        if map.len() >= self.config.max_entries && !map.contains_key(key) {
            return false; // at capacity; refuse new keys
        }
        map.insert(
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
}
