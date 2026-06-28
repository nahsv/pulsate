# 08. Cache

> The caching subsystem: pluggable stores (memory, disk, Redis), HTTP cache correctness (freshness, validators, Vary), invalidation by tag, background refresh and stale-while-revalidate, conditional and range requests, compression-aware caching, and cache observability.

**Contents**
- [Goals & model](#goals--model)
- [Stores: memory, disk, Redis](#stores-memory-disk-redis)
- [Cache keys & Vary](#cache-keys--vary)
- [Freshness & HTTP semantics](#freshness--http-semantics)
- [Conditional requests: ETag & Last-Modified](#conditional-requests-etag--last-modified)
- [Range requests](#range-requests)
- [Stale-while-revalidate & stale-if-error](#stale-while-revalidate--stale-if-error)
- [Background refresh](#background-refresh)
- [Invalidation & cache tags](#invalidation--cache-tags)
- [Compression-aware caching](#compression-aware-caching)
- [Cache metrics](#cache-metrics)
- [Cross-references](#cross-references)

---

## Goals & model

The cache exists to (1) cut latency and origin load for cacheable responses and (2) shield origins during incidents (serve stale on error). It is **correct-by-default**: it honors HTTP caching semantics (RFC 9111) rather than blindly caching, so enabling `cache` cannot silently serve private or stale-but-uncacheable content.

The cache is a **middleware** (`cache(@name)`) that does a lookup at [Ingress] (and may short-circuit with a hit) and a store at [Egress] (see [07. Middleware](07-middleware.md)). The store itself is a `CacheStore` trait (`pulsate-cache`) so memory/disk/Redis are interchangeable and a plugin can add new backends.

```
[Ingress] cache.on_request: key = compute_key(req)
            ├─ FRESH hit            → serve stored response (Stop) ──┐
            ├─ STALE hit + SWR      → serve stale, async revalidate ─┤→ [Egress] (still runs)
            ├─ STALE hit, must-rev. → conditional GET to origin       │
            └─ MISS                 → continue to handler ────────────┘
[Egress]  cache.on_response: if cacheable → store (with validators, tags, encodings)
```

## Stores: memory, disk, Redis

A named cache picks one store; routes reference it:

```
cache hot   { store memory { max 512MB; shards 16 } }                 # fastest, per-node
cache big   { store disk   { path "/var/cache/p8"; max 50GB } }    # large, survives restart
cache shared{ store redis  { url secret://redis_url; prefix "p8:" } } # cross-node coherence
```

| Store | Latency | Capacity | Scope | Survives restart |
|---|---|---|---|---|
| `memory` | ns–µs | RAM-bounded | per node | no (index can be persisted) |
| `disk` | µs–ms | TB | per node | yes |
| `redis` | network µs–ms | cluster | shared | yes |

- **Memory store:** sharded (to avoid a global lock), with an admission policy (TinyLFU-style) and a size-aware eviction (W-TinyLFU/LRU hybrid) so one-hit-wonders don't evict hot objects. Bodies stored as `Bytes`.
- **Disk store:** an index in `redb` ([23. Data & State Model](23-data-and-state-model.md)) mapping keys → on-disk blobs; bodies served via `sendfile` where TLS allows; periodic compaction.
- **Redis store:** for multi-node coherence (a purge on one node is seen by all). Used as a shared L2 behind the per-node memory L1 (tiered caching), so common objects are still served from RAM.
- **Tiered:** `store [memory { max 512MB }, redis { url ... }]` configures L1→L2; misses fall through, fills populate both.

## Cache keys & Vary

The cache key is composed explicitly so behavior is predictable and tunable:

```
cache c {
  key  [scheme, host, path, query, header.accept-encoding]   # default-ish key
  vary [accept-encoding, accept-language]                    # honor origin Vary too
}
```

- Default key = method + scheme + host + path + normalized query. `query` can be filtered (`query=[utm_*: ignore]`) so tracking params don't fragment the cache.
- **`Vary`** from the origin is honored automatically and merged with configured `vary`. Each (key, vary-tuple) is a distinct stored variant.
- Keys never include sensitive headers unless explicitly added; caching authenticated responses requires including an identity component (and Pulsate warns otherwise — see [07. Middleware](07-middleware.md)).

## Freshness & HTTP semantics

Pulsate implements RFC 9111 freshness:
- Computes freshness lifetime from `Cache-Control: max-age`/`s-maxage`, then `Expires`, then optional heuristic freshness (configurable, conservative).
- Respects `no-store`, `private`, `no-cache`, `must-revalidate`, `proxy-revalidate`, `s-maxage`.
- `default_ttl` applies only when the origin gives no freshness directive and heuristics are disabled.
- `Age` is computed and emitted; `Date` handling and clock-skew tolerance follow the spec.
- Methods cached: `GET`/`HEAD` by default; others are never cached. Responses with `Set-Cookie` are not shared-cached unless explicitly allowed.

## Conditional requests: ETag & Last-Modified

- On a **stale** stored entry, Pulsate revalidates with a conditional request to the origin using the stored `ETag` (`If-None-Match`) and/or `Last-Modified` (`If-Modified-Since`). A `304 Not Modified` refreshes the stored entry's freshness without re-transferring the body (big origin-bandwidth win).
- On the **client side**, Pulsate answers client conditional requests (`If-None-Match`/`If-Modified-Since`) from cache with `304` when the client's validator matches the stored entry — saving downstream bandwidth.
- Pulsate can also **generate** an `ETag` for origin responses that lack one (e.g., strong hash of the body) when configured, enabling conditional handling for naive origins.

## Range requests

- `Range`/`If-Range` are supported: Pulsate serves `206 Partial Content` from a fully-cached body, and for large objects can do **partial caching** — fetching and storing byte ranges and assembling them — so a video seek doesn't require caching the whole file first.
- Range requests coordinate with compression (see below) and validators (`If-Range` uses the stored ETag/Last-Modified).
- Multi-range requests are supported with `multipart/byteranges`.

## Stale-while-revalidate & stale-if-error

```
cache c { default_ttl 5m; stale_while_revalidate 30s; stale_if_error 5m }
```

- **`stale-while-revalidate`:** within the SWR window after expiry, Pulsate serves the stale response *immediately* and revalidates in the **background**, so users never wait on the origin for a freshness check. Honors the response's own `stale-while-revalidate` directive too.
- **`stale-if-error`:** if revalidation fails (origin 5xx, timeout, connect error), Pulsate serves the stale copy within the SIE window — turning the cache into an availability shield during incidents. This pairs with circuit breakers ([06. Reverse Proxy](06-reverse-proxy.md)).

## Background refresh

- **Async revalidation** for SWR runs on a background task (off the request path), coalesced so concurrent stale hits trigger exactly one origin revalidation (**request collapsing / single-flight**) rather than a thundering herd.
- **Proactive refresh** (optional): hot keys nearing expiry are refreshed ahead of time based on access frequency, so popular content is essentially always fresh in cache.
- **Cache warming:** an admin/CLI action (`p8 cache warm <urls>`) or config can pre-populate the cache after a deploy.

## Invalidation & cache tags

Beyond TTL, explicit invalidation:

- **Tags:** responses are tagged (from an origin `Cache-Tag`/`Surrogate-Key` header or a config rule), and a single purge invalidates all entries with a tag:
  ```
  cache c { tag_header "cache-tag" }     # origin sends: Cache-Tag: product-42, listing
  ```
  ```bash
  p8 cache purge --tag product-42       # CLI
  # or admin API: POST /v1/cache/purge {"tags":["product-42"]}
  ```
- **By key/URL/prefix:** `p8 cache purge --url https://x/y` or `--prefix /assets/`.
- **Purge-all:** `p8 cache purge --all` (scoped to a named cache).
- In a cluster, a purge propagates to all nodes (via the Redis store's pub/sub or the cluster bus — [16. Deployment](16-deployment.md)), so invalidation is fleet-wide and fast.
- **Soft purge:** mark stale (eligible for SIE) instead of hard-deleting, so a bad purge doesn't strip your incident shield.

## Compression-aware caching

Compression and caching interact subtly; Pulsate handles it correctly:
- Stored entries are keyed/varied on `Accept-Encoding` so a gzip client and an identity client get correct bodies.
- Pulsate can **store one canonical representation** (e.g., identity or brotli) and transcode on serve where safe, or store per-encoding variants — configurable to trade RAM for CPU.
- `compress` (Egress) and `cache` cooperate so a cached-then-compressed response isn't double-compressed and a range request over a compressed body is handled coherently (ranges apply to the transferred representation).
- `Vary: Accept-Encoding` is set automatically on compressed cached responses.

## Cache metrics

Exposed via [15. Observability](15-observability.md) / [26. Metrics Catalog](26-metrics-and-slo-catalog.md):
- `pulsate_cache_requests_total{result=hit|miss|stale|revalidated|bypass}`
- `pulsate_cache_hit_ratio` (derived), per cache and per route
- `pulsate_cache_bytes{store=memory|disk|redis}`, entries, evictions
- `pulsate_cache_revalidations_total{outcome=304|200|error}`
- `pulsate_cache_origin_saved_bytes_total` (bandwidth saved)
- `pulsate_cache_swr_served_total`, `pulsate_cache_stale_if_error_served_total`

The dashboard ([11. Dashboard](11-dashboard.md)) surfaces hit ratio, top keys, eviction pressure, and per-tag invalidation activity, making cache effectiveness legible at a glance.

## Cross-references
- [07. Middleware](07-middleware.md) — cache as Ingress lookup + Egress store; placement rules.
- [04. Configuration](04-configuration.md) — `cache {}` block and `cache(@name)` step.
- [06. Reverse Proxy](06-reverse-proxy.md) — stale-if-error + circuit breaker synergy.
- [23. Data & State Model](23-data-and-state-model.md) — disk cache index & on-disk layout.
- [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — full cache metric definitions.
