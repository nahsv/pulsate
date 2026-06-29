# 04. Configuration

> Pulsate Flow — the original configuration language. Its design principles, grammar, type system, and the full set of blocks (sites, routes, TLS, middleware, cache, WAF, load balancing, plugins, health checks, logging, metrics, auth, rate limiting, compression, CORS, headers), with many complete examples.

**Contents**
- [Why a new format](#why-a-new-format)
- [Design principles](#design-principles)
- [Lexical & grammar overview](#lexical--grammar-overview)
- [Value types](#value-types)
- [Top-level structure](#top-level-structure)
- [The flow operator and routes](#the-flow-operator-and-routes)
- [Sites & domains](#sites--domains)
- [TLS & ACME](#tls--acme)
- [Upstreams, load balancing & health checks](#upstreams-load-balancing--health-checks)
- [Middleware reference (built-ins)](#middleware-reference-built-ins)
- [Cache](#cache)
- [WAF & rate limiting](#waf--rate-limiting)
- [Auth](#auth)
- [Headers, CORS, compression](#headers-cors-compression)
- [Logging & metrics](#logging--metrics)
- [Plugins](#plugins)
- [Includes, env, secrets, variables](#includes-env-secrets-variables)
- [Validation & error reporting](#validation--error-reporting)
- [Worked examples](#worked-examples)
- [Cross-references](#cross-references)

---

## Why a new format

Existing formats each fail at least one of Pulsate's goals:
- **`nginx.conf`** is imperative, whitespace-and-semicolon fussy, and its directive semantics depend on context and module load order. Easy to write subtly wrong.
- **Caddyfile** is pleasant but its terseness hides ordering rules; JSON is the "real" config and is verbose.
- **Traefik** spreads config across YAML, labels, and CRDs — no single source of truth, and YAML's whitespace/typing footguns are well known.
- **Raw YAML/JSON** has no domain semantics, no comments (JSON), anchor/merge confusion, and no good error locations.

Pulsate Flow is a **purpose-built declarative DSL** designed around one insight: a gateway config is mostly *"for these requests, run this pipeline, then send them here."* The language makes that shape literal.

## Design principles

1. **The config reads like the request flows.** A route is a left-to-right pipeline (`~>`) from match → middleware → handler, mirroring the actual [request lifecycle](02-architecture.md#request-lifecycle). You read a route the way a packet experiences it.
2. **Typed, with units.** Durations (`30s`), sizes (`10MB`), counts, booleans, and references are first-class types — not stringly-typed. The parser validates types with precise locations.
3. **Declarative, never templated.** No loops, no string interpolation of the language itself, no Turing-completeness. Dynamism comes from typed values, env/secret refs, the admin API, and plugins — not from a templating engine.
4. **Safe defaults are implicit; danger is explicit.** Omitting TLS gives you automatic HTTPS. Disabling safety requires saying so out loud (`tls off`, `security { headers off }`).
5. **One file scales to many.** `use "..."` includes, named `@references`, and `defaults {}` let a config grow from five lines to a multi-tenant fleet without changing shape or tool.

## Lexical & grammar overview

Flow is parsed by a hand-written lexer + recursive-descent parser in [`pulsate-flow`](03-repository.md) so that every token carries a `(file, line, column, length)` span for diagnostics.

- **Comments:** `#` to end of line. (No block comments — keeps the lexer trivial and diffs clean.)
- **Blocks:** `keyword [args...] { ... }`. Braces delimit nesting; newlines separate statements; indentation is cosmetic (not significant).
- **Statements:** `keyword arg1 arg2 ...` — a directive with positional and/or named (`key=value`) arguments.
- **Routes:** the special pipeline form `route <matcher> ~> step ~> step ~> handler(...)`.
- **Strings:** `"double quoted"` with `\` escapes; bare unquoted tokens allowed for identifiers, hosts, paths, and references.
- **Trailing commas** allowed in arrays/argument lists.

Informal grammar (EBNF-ish):

```
config      = { statement } ;
statement   = block | directive | route ;
block       = ident { arg } "{" { statement } "}" ;
directive   = ident { arg } ;
route       = "route" matcher { "~>" step } ;
step        = ident [ "(" arglist ")" ] ;       # middleware or handler
arg         = value | ident "=" value ;          # positional or named
value       = string | number | duration | size | bool | array | ref | env | secret ;
ref         = "@" ident ;
env         = "${" ident [":-" default ] "}" ;
secret      = "secret://" ident ;
array       = "[" [ value { "," value } ] "]" ;
```

## Value types

| Type | Examples | Notes |
|---|---|---|
| String | `"GET"`, `app.example.com` | quoted or bare identifier |
| Integer | `100`, `8080` | |
| Float | `0.5`, `99.9` | for weights/percentages |
| Bool | `true`, `false`, `on`, `off` | `on/off` are aliases |
| Duration | `500ms`, `30s`, `5m`, `2h` | suffix required |
| Size | `4KB`, `10MB`, `1GB` | binary (KiB) by convention, documented |
| Rate | `100/min`, `10/s`, `1000/h` | count-per-window literal |
| Array | `["a","b"]`, `[GET, POST]` | homogeneous |
| Reference | `@api`, `@www_cache` | to a named block (upstream, cache, etc.) |
| Env | `${PORT:-8080}` | env var with optional default |
| Secret | `secret://db_password` | resolved by a secrets backend at load |

Invalid units, wrong types, and dangling references are **load-time errors** with the offending span.

## Top-level structure

A `pulsate.flow` is a sequence of top-level blocks. Order is irrelevant (the config is a set of declarations, resolved by reference), which avoids nginx-style "directive order matters" surprises.

```
flow_version "1"          # pins the config format version

pulsate { ... }             # engine/runtime/global settings
defaults { ... }          # reusable defaults applied to all sites/routes
upstream <name> { ... }   # a named backend pool (zero or more)
cache <name> { ... }      # a named cache (zero or more)
plugins { ... }           # plugin loading & config
cluster { ... }           # clustering (optional)
site <host...> { ... }    # a site = one or more hosts + its routes (one or more)
```

The `pulsate {}` engine block:

```
pulsate {
  http  { port 80 }                 # plain HTTP listener (used for redirects/ACME)
  https { port 443 }                # TLS listener
  http3 on                          # enable HTTP/3 on the https port
  workers 0                         # 0 = auto (one per core); N = prefork processes
  user  "pulsate"; group "pulsate"      # privilege drop after binding
  admin { listen 127.0.0.1:9180 }   # admin API/dashboard, loopback by default
  runtime { worker_threads 0; pin_workers off }
  shutdown { grace 30s }
}
```

## The flow operator and routes

A **route** is the signature construct. It binds a matcher to an ordered pipeline of middleware ending in exactly one **handler**:

```
route <matcher> ~> <middleware> ~> <middleware> ~> <handler(args)>
```

- **Matcher** selects requests: a path pattern, optionally refined by method/host/header/query predicates.
- **Middleware** steps run during [Ingress] in written order and during [Egress] in reverse (see [07. Middleware](07-middleware.md)).
- **Handler** is terminal — exactly one per route — e.g. `proxy(@api)`, `files("/var/www")`, `redirect(...)`, `respond(...)`, `grpc(@svc)`, `ws(@chat)`.

Matchers:

```
route /api/*                      # path prefix (glob)
route = /healthz                  # exact path (the `=` prefix)
route ~ ^/u/(?<id>\d+)$           # regex (the `~` prefix), named captures usable downstream
route /api/* [method=POST]        # refine by method
route /* [host=admin.example.com] # refine by host (within a multi-host site)
route /* [header.x-canary=true]   # refine by header
route /search [query.q=*]         # refine by query presence
```

Routes within a site are evaluated **most-specific-first** (exact > longer prefix > regex > catch-all), a deterministic order documented in [06. Reverse Proxy](06-reverse-proxy.md) — no reliance on declaration order.

## Sites & domains

A `site` groups one or more hostnames and the routes served for them.

```
site example.com www.example.com {       # multiple hosts share routes
  tls auto                               # automatic HTTPS (ACME)
  route /* ~> compress ~> files("/srv/www")
}

site *.preview.example.com {             # wildcard host
  tls auto
  route /* ~> proxy(@preview)
}
```

Host matching supports exact, leading-wildcard (`*.example.com`), and a catch-all `site :default { ... }` for unmatched hosts.

## TLS & ACME

Secure by default: a `site` with no `tls` directive still gets automatic HTTPS.

```
# Automatic (ACME) — the default
site shop.example.com {
  tls auto                               # provider/email taken from defaults or pulsate{}
}

# Explicit ACME settings
acme {
  provider letsencrypt                   # or zerossl, or a custom directory URL
  email ops@example.com
  challenge http-01                      # http-01 | tls-alpn-01 | dns-01
  # dns provider for dns-01 / wildcards:
  dns { provider cloudflare; api_token secret://cf_token }
}

# Manual certificates
site internal.example.com {
  tls {
    cert "/etc/pulsate/certs/internal.crt"
    key  secret://internal_key
    min_version 1.2
    ciphers modern                       # modern | intermediate | custom list
    client_auth { mode require; ca "/etc/pulsate/ca.pem" }   # mTLS
  }
}

# Disable TLS (explicit, discouraged)
site dev.localhost { tls off; route /* ~> proxy(@local) }
```

ACME, cert storage, renewal, OCSP stapling, and mTLS verification are detailed in [09. Security](09-security.md) and [05. HTTP Stack](05-http-stack.md).

## Upstreams, load balancing & health checks

A named pool of backends with a balancing policy and health checking:

```
upstream api {
  target http://10.0.0.11:8080 weight=3
  target http://10.0.0.12:8080 weight=1
  target https://10.0.0.13:8443                 # mixed schemes allowed

  policy        least_conn          # round_robin | least_conn | ip_hash | random | ewma
  sticky        { cookie "psid"; ttl 1h }        # session affinity (optional)

  retry         { attempts 2; on [502,503,504, connect_error]; budget 20% }
  breaker       { window 30s; threshold 50%; min_requests 20; open_for 15s }

  health {
    active   { path /healthz; interval 5s; timeout 2s; healthy 2; unhealthy 3 }
    passive  { on [502,503,504, connect_error]; eject_after 5; eject_for 30s }
  }

  pool      { max_idle 64; idle_timeout 90s; max_per_host 256 }
  timeouts  { connect 2s; response 30s; idle 60s }
}

# Dynamic discovery instead of static targets:
upstream svc {
  discover dns "api.svc.cluster.local" { refresh 10s }    # or: kubernetes, consul
  policy ewma
}
```

Semantics (algorithms, retry budgets, breaker states, discovery) are specified in [06. Reverse Proxy](06-reverse-proxy.md).

## Middleware reference (built-ins)

Middleware appear as steps in a route's `~>` chain. Built-ins (full semantics in [07. Middleware](07-middleware.md)):

| Step | Purpose |
|---|---|
| `compress` / `compress(br, gzip)` | response compression, negotiated |
| `cors` / `cors(origins=[...], methods=[...])` | CORS handling |
| `headers(set={...}, remove=[...])` | request/response header rewriting |
| `rate_limit(100/min, key=ip)` | rate limiting |
| `jwt(aud=, iss=, jwks=)` | JWT validation |
| `basic_auth(users=@team)` | HTTP basic auth |
| `forward_auth(@authsvc)` | external auth subrequest |
| `cache(@www_cache)` | enable caching for the route |
| `waf(@ruleset)` | apply a WAF ruleset |
| `rewrite(path=...)` / `strip_prefix("/api")` | path rewriting |
| `redirect(to=..., status=308)` | (also usable as a terminal handler) |
| `retry(...)` | per-route retry override |
| `timeout(10s)` | per-route timeout |
| `plugin.<name>(...)` | a loaded WASM plugin used as middleware |

Handlers (terminal):

| Handler | Purpose |
|---|---|
| `proxy(@upstream)` | reverse proxy to a pool |
| `files("/path")` | static file server |
| `redirect(to=, status=)` | redirect response |
| `respond(status=, body=)` | inline/static response |
| `grpc(@upstream)` | gRPC proxy |
| `ws(@upstream)` | WebSocket proxy |
| `plugin.<name>(...)` | a plugin acting as a handler |

## Cache

A named cache defines a store and policy; routes opt in with `cache(@name)`:

```
cache www_cache {
  store     memory { max 512MB }            # memory | disk { path, max } | redis { url }
  # store   disk { path "/var/cache/pulsate"; max 10GB }
  # store   redis { url secret://redis_url }

  default_ttl   5m
  methods       [GET, HEAD]
  key           [host, path, query, header.accept-encoding]   # cache key composition
  stale_while_revalidate 30s
  stale_if_error         5m
  vary          [accept-encoding]
  ignore_no_cache false
}

site cdn.example.com {
  tls auto
  route /assets/* ~> cache(@www_cache) ~> proxy(@origin)
}
```

Conditional requests (ETag/Last-Modified), range requests, tag-based invalidation, background refresh, and compression-aware caching are specified in [08. Cache](08-cache.md).

## WAF & rate limiting

```
waf default {
  mode      block                 # block | detect (log only)
  rules     [owasp_crs, custom]   # built-in rulesets + named custom rules
  custom    "/etc/pulsate/waf/*.rules"
  geo       { block [RU, KP]; allow [] }     # ISO country codes
  asn       { block [AS13335] }
  bot       { mode challenge; allow [googlebot] }   # challenge | block | allow
  ip        { deny ["203.0.113.0/24"]; allow ["10.0.0.0/8"] }
}

site api.example.com {
  tls auto
  route /* ~> waf(@default)
          ~> rate_limit(1000/min, key=ip)
          ~> proxy(@api)
}
```

Rate limiting supports composite keys (`key=ip`, `key=header.x-api-key`, `key=[ip, path]`), multiple windows, and a distributed mode backed by the cluster/Redis. WAF internals are in [09. Security](09-security.md).

## Auth

```
# JWT
route /api/* ~> jwt(iss="https://issuer", aud="api", jwks="https://issuer/.well-known/jwks.json")
            ~> proxy(@api)

# Basic auth with a user set
users team { user "alice" secret://alice_pw; user "bob" secret://bob_pw }
route /admin/* ~> basic_auth(users=@team) ~> proxy(@admin)

# External forward-auth (delegated decision)
route /* ~> forward_auth(@authsvc, copy_headers=[x-user, x-roles]) ~> proxy(@app)

# mTLS at the TLS layer (see TLS block); plus a claims check:
route /* ~> require(cert.cn in ["svc-a","svc-b"]) ~> proxy(@app)
```

## Headers, CORS, compression

```
defaults {
  headers {
    set { x-frame-options "DENY"; strict-transport-security "max-age=31536000; includeSubDomains" }
    remove [server, x-powered-by]
  }
  compress br gzip { min_size 1KB; types [text/*, application/json, application/javascript] }
}

site app.example.com {
  tls auto
  route /api/* ~> cors(origins=["https://app.example.com"], methods=[GET,POST], credentials=true)
              ~> proxy(@api)
}
```

Security headers are applied secure-by-default from `defaults`; per-route `headers(...)` can add/override.

## Logging & metrics

```
log {
  level   info                      # error|warn|info|debug|trace
  format  json                      # json | text
  output  stdout                    # stdout | file "/var/log/pulsate.log" | both
  access  { enabled true; fields [ts, method, host, path, status, dur_ms, upstream, req_id] }
  sample  { success 0.1 }           # sample 10% of 2xx access logs (errors always logged)
}

metrics {
  prometheus { listen 127.0.0.1:9100; path /metrics }
  otel       { endpoint "http://otel-collector:4317"; protocol grpc }
}

tracing {
  enabled true
  exporter otlp
  sample   0.05                     # 5% head-based sampling
  propagate [w3c, b3]
}
```

Metric names, trace spans, and request-ID propagation are catalogued in [15. Observability](15-observability.md) and [26. Metrics Catalog](26-metrics-and-slo-catalog.md).

## Plugins

```
plugins {
  dir "/etc/pulsate/plugins"          # where .wasm components live
  load geoblock { source "geoblock.wasm"; config { db "/etc/geo.mmdb" } }
  load mytransform { source "oci://registry.example.com/plugins/transform:1.2.0" }
}

site app.example.com {
  tls auto
  route /* ~> plugin.geoblock(allow=[US, CA])
          ~> plugin.mytransform(mode="rewrite")
          ~> proxy(@app)
}
```

Sandboxing, capability grants, versioning, and the SDK are in [12. Plugins](12-plugins.md).

## Includes, env, secrets, variables

```
flow_version "1"
use "tls.flow"                      # include another file (merged at load)
use "sites/*.flow"                  # glob include (multi-tenant friendly)

let cdn_origin = http://origin.internal:8080     # a named value (not templating)

upstream origin { target ${ORIGIN_URL:-http://localhost:8080} }

pulsate { https { port ${HTTPS_PORT:-443} } }

acme { dns { provider cloudflare; api_token secret://cf_token } }
```

- `use` merges files; duplicate definitions are a load error (no silent override).
- `${VAR:-default}` reads environment with an optional default.
- `secret://name` is resolved by the configured [secrets backend](09-security.md) at load and on rotation — secrets never appear literally in the file.
- `let` binds a reusable value; it is *not* string templating (no concatenation gymnastics), just a typed alias.

## Validation & error reporting

`pulsate validate pulsate.flow` (and every reload) runs the full pipeline from [02. Architecture](02-architecture.md#configuration-loading). Errors are rendered with the span and a fix hint:

```
error[PLS-CFG-0007]: unknown upstream reference
  ┌─ pulsate.flow:14:31
  │
14 │   route /api/* ~> proxy(@apii)
  │                         ^^^^^ no upstream named `apii` is defined
  │
  = help: did you mean `@api`? (defined at pulsate.flow:3:10)
```

Validation covers: syntax, types/units, required fields, referential integrity (`@refs`, plugins), and invariants (no host+port collision, ACME reachability, exactly one handler per route).

## Worked examples

**1. The 30-second start (a Rails app, auto-HTTPS):**
```
site myapp.com www.myapp.com {
  tls auto
  route /* ~> proxy(http://localhost:3000)
}
```

**2. SPA + API split with caching and CORS:**
```
upstream api { target http://127.0.0.1:8080; policy least_conn; health { active { path /healthz; interval 5s } } }
cache assets { store memory { max 256MB }; default_ttl 1h; stale_while_revalidate 30s }

site app.example.com {
  tls auto
  route /api/* ~> cors(origins=["https://app.example.com"], credentials=true)
              ~> rate_limit(600/min, key=ip)
              ~> strip_prefix("/api")
              ~> proxy(@api)
  route /assets/* ~> cache(@assets) ~> compress ~> files("/srv/app/assets")
  route /*        ~> files("/srv/app", try=["{path}", "/index.html"])   # SPA fallback
}
```

**3. Microservices gateway with gRPC, WebSocket, weighted canary:**
```
upstream checkout_v1 { target http://10.0.0.1:8080 weight=9 }
upstream checkout_v2 { target http://10.0.0.2:8080 weight=1 }
upstream chat { target http://10.0.0.5:9000 }
upstream billing { target grpc://10.0.0.7:50051 }

site gw.example.com {
  tls auto
  route /checkout/* ~> waf(@default) ~> proxy(split=[@checkout_v1, @checkout_v2])   # weighted canary
  route /chat       ~> ws(@chat)
  route /billing.*  ~> jwt(aud="internal") ~> grpc(@billing)
  route /*          ~> respond(status=404, body="not found")
}
```

**4. Hardened public API (WAF, geo, mTLS to upstream, headers):**
```
waf strict { mode block; rules [owasp_crs]; geo { block [KP] }; bot { mode challenge } }

upstream backend {
  target https://10.1.0.10:8443
  tls { client_cert "/etc/pulsate/id.crt"; client_key secret://id_key; ca "/etc/pulsate/upstream-ca.pem" }
}

site api.example.com {
  tls { cert "/etc/pulsate/api.crt"; key secret://api_key; min_version 1.3; client_auth { mode request; ca "/etc/pulsate/clients-ca.pem" } }
  route /* ~> waf(@strict)
          ~> rate_limit(2000/min, key=[ip, header.x-api-key])
          ~> headers(set={ strict-transport-security: "max-age=63072000" }, remove=[server])
          ~> jwt(iss="https://issuer", aud="api", jwks="https://issuer/jwks.json")
          ~> proxy(@backend)
}
```

**5. Multi-tenant fleet via includes:**
```
flow_version "1"
use "defaults.flow"
use "tenants/*.flow"     # each tenant ships its own site{} block; isolation per 29-multi-tenancy
metrics { prometheus { listen 0.0.0.0:9100 } }
```

## Cross-references
- [02. Architecture](02-architecture.md) — how Flow becomes a `ConfigSnapshot`.
- [06. Reverse Proxy](06-reverse-proxy.md) — routing precedence, LB, retries, breakers, discovery.
- [07. Middleware](07-middleware.md) — `~>` pipeline semantics and built-ins.
- [08. Cache](08-cache.md), [09. Security](09-security.md) — cache and WAF/auth/TLS internals.
- [27. Configuration Reference](27-config-reference.md) — exhaustive key-by-key reference.
- [30. Migration & Import](30-migration-and-import.md) — importing nginx/Caddy/Traefik configs to Flow.
