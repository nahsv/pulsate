# 27. Configuration Reference

> The exhaustive, section-by-section reference for the Pulsate Flow format: every block, directive, and field with its type, default, scope, and constraints. This complements the conceptual treatment in [04. Configuration](04-configuration.md) and is generated from the config schema so it never drifts.

**Contents**
- [How to read this](#how-to-read-this)
- [Top-level](#top-level)
- [`pulsate {}` engine](#pulsate--engine)
- [`defaults {}`](#defaults-)
- [`site {}`](#site-)
- [`route`](#route)
- [`upstream {}`](#upstream-)
- [`tls {}` / `acme {}`](#tls---acme-)
- [`cache {}`](#cache-)
- [`waf {}`](#waf-)
- [Middleware directives](#middleware-directives)
- [`log {}` / `metrics {}` / `tracing {}`](#log---metrics---tracing-)
- [`plugins {}`](#plugins-)
- [`cluster {}`](#cluster-)
- [Types & global directives](#types--global-directives)
- [Cross-references](#cross-references)

---

## How to read this

Each field is listed as `name : type = default` with scope and notes. Types are from [04. Configuration — value types](04-configuration.md#value-types) (duration, size, rate, ref `@x`, env `${X}`, secret `secret://x`). "Required" fields have no default. Unknown keys are `PLS-CFG-0002` errors; type mismatches are `PLS-CFG-0003` ([25. Error Catalog](25-error-and-status-catalog.md)). This reference is **generated** from the schema that drives validation, so it is authoritative for the binary version it ships with.

## Top-level

| Directive | Type | Default | Notes |
|---|---|---|---|
| `flow_version` | string | — (required) | config format version; binary supports a range |
| `use` | string/glob | — | include file(s); duplicate defs → `PLS-CFG-0020` |
| `let <name>` | value | — | typed named alias (not templating) |
| `pulsate {}` | block | implied defaults | engine settings |
| `defaults {}` | block | — | inherited defaults for sites/routes |
| `upstream <name> {}` | block | — | a backend pool |
| `cache <name> {}` | block | — | a named cache |
| `waf <name> {}` | block | — | a WAF ruleset |
| `users <name> {}` | block | — | a basic-auth user set |
| `site <host...> {}` | block | — | a site |
| `plugins {}` | block | — | plugin loading |
| `cluster {}` | block | off | clustering |
| `log/metrics/tracing {}` | block | sensible defaults | observability |

## `pulsate {}` engine

| Field | Type | Default | Notes |
|---|---|---|---|
| `http { port }` | int | 80 | plain HTTP (redirect/ACME) |
| `https { port }` | int | 443 | TLS listener |
| `http3` | bool | true | enable H3 on the https port (UDP) |
| `workers` | int | 0 | 0=auto (per core); N=prefork processes |
| `user` / `group` | string | — | drop privileges after bind |
| `data_dir` | string | `/var/lib/p8` | durable state ([23](23-data-and-state-model.md)) |
| `admin { listen }` | addr | `127.0.0.1:9180` | admin API/dashboard (loopback default) |
| `admin { auth }` | block | required if non-loopback | tokens/oidc/mtls |
| `runtime { worker_threads }` | int | 0 (auto) | Tokio threads |
| `runtime { pin_workers }` | bool | false | CPU affinity |
| `shutdown { grace }` | duration | 30s | drain deadline |
| `limits { max_connections, max_header_bytes, max_body_bytes }` | int/size | bounded defaults | global guards |
| `timeouts { handshake, header_read, request, idle, stream_idle }` | duration | per [05](05-http-stack.md) | global timeouts |

## `defaults {}`

Holds any `headers {}`, `compress`, `tls`, `acme`, security headers, and timeout defaults inherited by all sites/routes unless overridden. Same field set as the corresponding blocks below.

## `site {}`

| Field | Type | Default | Notes |
|---|---|---|---|
| (positional) `<host...>` | string(s) | — | exact, `*.wildcard`, or `:default` |
| `tls` | `auto`/`off`/block | `auto` | see TLS |
| `route ...` | route | — | one or more routes |
| `header_inherit` | bool | true | inherit `defaults` |

## `route`

`route <matcher> [predicates] ~> step ~> ... ~> handler(args)`

| Element | Forms |
|---|---|
| matcher | `/prefix/*`, `= /exact`, `~ ^regex$` |
| predicates | `[method=…]`,`[host=…]`,`[header.x=…]`,`[query.q=…]` (comma-separated) |
| steps | any middleware directive (below) |
| handler | exactly one of `proxy`,`files`,`redirect`,`respond`,`grpc`,`ws`,`plugin.<n>` (`PLS-CFG-0012` if 0 or >1) |

## `upstream {}`

| Field | Type | Default | Notes |
|---|---|---|---|
| `target <url>` | url (+`weight=`) | — | one+ static targets |
| `discover <provider> {}` | block | — | dns/kubernetes/consul/plugin |
| `policy` | enum | `least_conn` | `round_robin|least_conn|ewma|ip_hash|random` |
| `sticky {}` | block | off | `cookie|ip_hash|header`, `ttl`, `fallback` |
| `retry {}` | block | off | `attempts,on,methods,budget,backoff,per_try_timeout` |
| `breaker {}` | block | off | `window,threshold,min_requests,open_for,half_open` |
| `health { active{} passive{} }` | block | off | probes & ejection |
| `pool {}` | block | defaults | `max_idle,idle_timeout,max_per_host` |
| `timeouts {}` | block | global | `connect,response,idle` |
| `tls {}` | block | — | upstream TLS/mTLS (`client_cert,client_key,ca,sni`) |

## `tls {}` / `acme {}`

`tls` field: `auto` | `off` | block:

| Field | Type | Default |
|---|---|---|
| `cert` / `key` | path / path|secret | — (manual) |
| `min_version` | enum | 1.2 |
| `ciphers` | enum/list | `intermediate` |
| `client_auth { mode, ca }` | block | off (`request|require`) |
| `alpn` | list | auto |

`acme` block:

| Field | Type | Default |
|---|---|---|
| `provider` | enum/url | letsencrypt |
| `email` | string | — (recommended) |
| `challenge` | enum | `http-01` |
| `dns { provider, api_token }` | block | — (for dns-01/wildcard) |
| `on_demand { allow [...] }` | block | off |

## `cache {}`

| Field | Type | Default |
|---|---|---|
| `store` | block/list | required (`memory{max}`/`disk{path,max}`/`redis{url}`/tiered list) |
| `default_ttl` | duration | — |
| `methods` | list | `[GET,HEAD]` |
| `key` | list | `[scheme,host,path,query]` |
| `vary` | list | merged with origin Vary |
| `stale_while_revalidate` / `stale_if_error` | duration | 0 |
| `tag_header` | string | — |
| `ignore_no_cache` | bool | false |

## `waf {}`

| Field | Type | Default |
|---|---|---|
| `mode` | enum | `block` (`block|detect`) |
| `rules` | list | `[owasp_crs]` |
| `custom` | path/glob | — |
| `geo { block, allow }` | block | — |
| `asn { block, allow }` | block | — |
| `bot { mode, allow, deny }` | block | — |
| `ip { allow, deny }` | block | — |
| `anomaly { threshold }` | block | CRS-like |

## Middleware directives

| Directive | Key args |
|---|---|
| `compress` | codecs, `min_size`, `types` |
| `cors` | `origins,methods,headers,credentials,max_age` |
| `headers` | `set={}`,`remove=[]` |
| `rate_limit` | `<rate>`,`key=`,`burst=` |
| `jwt` | `iss,aud,jwks,algorithms,leeway,forward_claims` |
| `basic_auth` | `users=@set` |
| `forward_auth` | `@svc`,`copy_headers` |
| `require` | `<expr>` |
| `cache` | `@cache` |
| `waf` | `@ruleset` |
| `rewrite`/`strip_prefix` | `path=…` / `"<prefix>"` |
| `timeout`/`retry` | per-route overrides |
| `on_error` | `<handler>` |
| `plugin.<name>` | plugin-defined |

## `log {}` / `metrics {}` / `tracing {}`

`log`: `level,format(json|text),output,access{enabled,fields},sample{success}`.
`metrics`: `prometheus{listen,path}`, `otel{endpoint,protocol}`.
`tracing`: `enabled,exporter,endpoint,sample,propagate[w3c|b3]`.

## `plugins {}`

| Field | Type | Default |
|---|---|---|
| `dir` | path | — |
| `load <name> { source, config{}, capabilities{} }` | block | — |
| `require_signed` | bool | false |
| `trusted_keys` | list | — |

`capabilities`: `net [hosts]`,`kv read|write|none`,`env [...]`,`fs none|path`.

## `cluster {}`

| Field | Type | Default |
|---|---|---|
| `id` | string | — |
| `peers` / `discover` | list/block | — |
| `bind` | addr | — |
| `state { certs, cache, rate_limit, sticky }` | block | per-node |
| `backend` | enum | gossip (`gossip|etcd|consul`) |

## Types & global directives

- **Durations:** `ms,s,m,h`. **Sizes:** `KB,MB,GB`. **Rates:** `<n>/s|min|h`.
- **References:** `@name` to upstream/cache/waf/users; dangling → `PLS-CFG-0007`.
- **Env/secret:** `${VAR:-default}`, `secret://name` (resolved at load + rotation).
- **Validation invariants:** unique host+port; one handler/route; ACME reachability; bounded limits — full list in [04. Configuration](04-configuration.md#validation--error-reporting).

## Cross-references
- [04. Configuration](04-configuration.md) — the conceptual guide and examples.
- [25. Error Catalog](25-error-and-status-catalog.md) — `PLS-CFG-*` codes for violations.
- [06](06-reverse-proxy.md)/[08](08-cache.md)/[09](09-security.md) — semantics behind these fields.
- [30. Migration & Import](30-migration-and-import.md) — mapping foreign configs to these keys.
