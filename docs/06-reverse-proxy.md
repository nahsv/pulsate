# 06. Reverse Proxy

> The proxy core: how routes are matched, how requests are rewritten and forwarded, how upstreams are balanced, retried, circuit-broken, health-checked, and discovered — i.e. the [Match], [Dispatch], and [Upstream] lifecycle stages in full.

**Contents**
- [Routing engine](#routing-engine)
- [Match precedence](#match-precedence)
- [Host, path, regex, and predicate routing](#host-path-regex-and-predicate-routing)
- [Header & request rewriting](#header--request-rewriting)
- [Weighted & canary routing](#weighted--canary-routing)
- [Sticky sessions](#sticky-sessions)
- [Load balancing policies](#load-balancing-policies)
- [Retry policies](#retry-policies)
- [Circuit breakers](#circuit-breakers)
- [Health checks](#health-checks)
- [Service discovery & dynamic upstreams](#service-discovery--dynamic-upstreams)
- [Cross-references](#cross-references)

---

## Routing engine

At snapshot-build time, `pulsate-router` compiles all sites and routes into an immutable **routing table** optimized for fast, allocation-free matching at request time:

```
ConfigSnapshot
└── routing table
    ├── host index            # exact map + wildcard suffix tree + :default
    │     └── per-host route set
    │           ├── exact-path map        (O(1))
    │           ├── prefix trie           (longest-prefix, O(path-length))
    │           ├── regex set             (compiled, evaluated in precedence order)
    │           └── predicate filters     (method/header/query refinements)
    └── each leaf → (middleware list, handler), all Arc-shared
```

At **[Match]** the engine resolves host → route deterministically with no heap allocation and no locks (it reads the immutable table via the request's snapshot `Arc`). The result is a `RouteMatch` carrying the middleware list, the handler, and any captured variables (regex named groups, path segments).

## Match precedence

Routes do **not** depend on declaration order (unlike nginx `location` subtleties). Precedence is fixed and documented so behavior is predictable:

1. **Host specificity:** exact host > wildcard host (`*.example.com`) > `:default`.
2. **Within a host, path specificity:**
   1. exact (`= /healthz`)
   2. longest matching prefix (`/api/v2/*` beats `/api/*`)
   3. regex (`~ ...`), evaluated in source order among regexes
   4. catch-all (`/*` or `/`)
3. **Predicate refinement:** among equally-specific paths, a route with matching `[method=…]`/`[header.…]`/`[query.…]` predicates beats an unrefined one; if multiple match, the first in source order wins and a **load-time lint warns** about the ambiguity.

`pulsate validate` reports unreachable routes (shadowed by a more specific one) as warnings.

## Host, path, regex, and predicate routing

```
site api.example.com admin.example.com {     # two hosts, shared + host-specific routes
  tls auto

  route = /healthz            ~> respond(status=200, body="ok")     # exact
  route /v2/*                 ~> proxy(@api_v2)                      # longer prefix wins
  route /*                    ~> proxy(@api_v1)                      # prefix
  route ~ ^/u/(?<id>\d+)$     ~> proxy(@users)                       # regex w/ capture {id}
  route /* [host=admin.example.com] ~> proxy(@admin)                # host predicate
  route /* [method=OPTIONS]   ~> cors ~> respond(status=204)        # method predicate
}
```

- **Captures** from regex (`{id}`) and path globs are available to downstream steps (e.g., `rewrite(path="/internal/{id}")`).
- **Case-sensitivity, trailing-slash, and percent-decoding** behavior are explicit, documented defaults (paths matched on the normalized, decoded path; configurable strictness).

## Header & request rewriting

Rewriting happens as middleware in the [Ingress] phase (request) and [Egress] (response):

```
route /api/* ~> strip_prefix("/api")                       # /api/x -> /x to upstream
            ~> rewrite(path="/v2{path}")                    # add a prefix using capture
            ~> headers(
                 set={ x-forwarded-proto: "https",
                       x-real-ip: "{client.ip}",
                       host: "backend.internal" },          # override Host sent upstream
                 remove=[cookie] )
            ~> proxy(@api)
```

- **Forwarded headers:** Pulsate sets `X-Forwarded-For`/`-Proto`/`-Host` and RFC 7239 `Forwarded` by default (configurable), and *trusts* inbound forwarded headers only from configured trusted proxies (security-relevant — see [09. Security](09-security.md)).
- **Host header policy:** preserve client Host, or override per route/upstream.
- **Response rewriting:** add/remove/override response headers, rewrite `Location`/`Set-Cookie` domains for transparent proxying.
- **Body rewriting** is possible via middleware/plugins but off the default path (streaming preserved unless explicitly transforming).

## Weighted & canary routing

Two complementary mechanisms:

- **Weighted targets within one upstream** (infra-level balancing):
  ```
  upstream api { target http://a:8080 weight=8; target http://b:8080 weight=2 }
  ```
- **Weighted split across upstreams** (release-level canary), expressed at the handler:
  ```
  route /checkout/* ~> proxy(split=[@checkout_v1 :90, @checkout_v2 :10])
  ```
  Splits can be **sticky by key** (so a user consistently sees one variant): `proxy(split=[...], key=cookie.uid)`. Header/cookie-based pinning enables blue/green and progressive delivery; combined with metrics this supports automated canary analysis ([20. Future](20-future.md)).

## Sticky sessions

When a backend holds session state, affinity keeps a client on one target:

```
upstream app {
  target http://a:8080; target http://b:8080
  sticky { cookie "psid"; ttl 1h; fallback rebalance }   # cookie | ip_hash | header
}
```

- **Cookie mode:** Pulsate sets a signed affinity cookie naming the chosen backend (the backend identity is encoded/HMAC'd, not leaked).
- **Hash mode:** `ip_hash` or a header hash maps a key to a backend consistently.
- **Failover:** if the sticky target is unhealthy, `fallback` chooses `rebalance` (pick a new healthy target, re-pin) or `fail` (return error) per policy.
- Consistent hashing (with bounded loads) minimizes reshuffling when the backend set changes.

## Load balancing policies

Configured per upstream (`policy …`); all operate over the *currently healthy* target set:

| Policy | Behavior | Good for |
|---|---|---|
| `round_robin` | even rotation (weighted) | uniform backends |
| `least_conn` | fewest in-flight requests | uneven request cost |
| `ewma` | lowest exponentially-weighted moving-average latency (power-of-two-choices) | latency-sensitive, heterogeneous backends |
| `ip_hash` | client-IP → backend | crude affinity without cookies |
| `random` / `p2c` | random / power-of-two-choices random | simple, surprisingly good tail behavior |

Weights apply to `round_robin`/`random`. The default is `least_conn` (robust general choice). Selection is O(1)/O(log n) and lock-light (per-target atomic counters in a sharded registry).

## Retry policies

```
upstream api {
  retry {
    attempts 2                                  # total tries = 1 + 2 retries
    on [502, 503, 504, connect_error, reset]    # retriable conditions
    methods [GET, HEAD, PUT, DELETE]            # idempotent by default; opt-in others carefully
    budget 20%                                  # cap retries to 20% of requests (retry storms)
    backoff { base 50ms; max 1s; jitter true }
    per_try_timeout 5s
  }
}
```

- **Idempotency-aware:** by default only idempotent methods retry; non-idempotent retries require explicit opt-in and are never retried after bytes have been sent upstream (avoids duplicate side effects).
- **Retry budgets** bound retries as a fraction of traffic so a struggling backend isn't hammered into collapse (defense against retry storms).
- **Hedging** (optional): for read-only requests, a second attempt may be raced after a delay to cut tail latency, with the budget still applied.
- Each retry picks a *different* healthy target (avoids the one that just failed).

## Circuit breakers

A per-target breaker prevents a failing backend from absorbing traffic:

```
breaker { window 30s; threshold 50%; min_requests 20; open_for 15s; half_open 3 }
```

State machine:

```
 CLOSED ──(error rate ≥ threshold over window, ≥ min_requests)──▶ OPEN
   ▲                                                              │
   │                                          after `open_for`    ▼
 (probes succeed) ◀────────── HALF_OPEN ◀──────────────── (allow `half_open` probes)
   │                              │
   └──── (probe fails) ───────────┘ ──▶ back to OPEN
```

- **OPEN:** requests fail fast (`503`, `Retry-After`) or fall back (next target / configured fallback handler) without touching the backend.
- **HALF_OPEN:** a few probe requests test recovery; success closes the breaker, failure re-opens it.
- Breaker state is per-target, sharded, and surfaced as metrics and on the dashboard ([11. Dashboard](11-dashboard.md)). Passive health ejection (below) feeds the same signal.

## Health checks

Two layers, used together:

- **Active** — Pulsate proactively probes each target:
  ```
  health { active { path /healthz; interval 5s; timeout 2s; healthy 2; unhealthy 3;
                    expect { status 200; body ~ "ok" } } }
  ```
  A target flips healthy/unhealthy after N consecutive successes/failures (hysteresis avoids flapping). Probes can use a different port/protocol (e.g., gRPC health checking protocol).
- **Passive** — real traffic outcomes eject misbehaving targets:
  ```
  health { passive { on [502,503,504,connect_error]; eject_after 5; eject_for 30s } }
  ```
  After repeated failures a target is temporarily ejected and periodically re-admitted (like Envoy outlier detection, generalized).

The **healthy set** (active-healthy ∧ not-passively-ejected ∧ breaker-not-open) is what the LB policy selects from. If *all* targets are unhealthy, configurable behavior: `fail` (return 503) or `panic_route` (serve from any target — "fail open" for availability-critical paths).

## Service discovery & dynamic upstreams

Static targets are one source; dynamic discovery keeps the target set live without reloads:

```
upstream svc { discover dns "api.svc.cluster.local" { refresh 10s; port 8080 } }
upstream k8s { discover kubernetes { service "api"; namespace "prod"; port "http" } }
upstream cns { discover consul { service "api"; tag "prod"; refresh 5s } }
```

- **Providers:** DNS (A/AAAA/SRV), Kubernetes (EndpointSlices via the API/CRD integration — [14. DX](14-developer-experience.md)), Consul, and plugin-provided discoverers.
- **Dynamic membership:** discovered targets are merged into the upstream's live target set; additions start in a warming state and pass health checks before receiving full traffic; removals drain.
- **No reload required:** discovery updates mutate the live pool registry (the same registry that survives reloads), so endpoints can churn rapidly while the `ConfigSnapshot` stays stable.
- **Admin visibility:** current targets, weights, health, and breaker state per upstream are queryable via the [22. Admin API](22-admin-api.md) and shown on the dashboard.

## Cross-references
- [02. Architecture](02-architecture.md) — Match/Dispatch/Upstream stages and the snapshot/registry split.
- [04. Configuration](04-configuration.md) — route, upstream, health, retry, breaker syntax.
- [05. HTTP Stack](05-http-stack.md) — upstream pooling, keep-alive, gRPC/WebSocket proxying.
- [07. Middleware](07-middleware.md) — where rewriting/auth/ratelimit sit relative to Dispatch.
- [10. Performance](10-performance.md) — lock-light selection and pool design.
- [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — proxy/upstream/breaker metrics.
