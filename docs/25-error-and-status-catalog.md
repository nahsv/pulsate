# 25. Error and Status Catalog

> The canonical error taxonomy: stable `PLS-*` codes, their categories, HTTP-status and exit-code mappings, the `application/problem+json` response shape, and how errors flow through the [Recover] phase. Every error Pulsate can emit is discoverable and documented.

**Contents**
- [Why a stable taxonomy](#why-a-stable-taxonomy)
- [Code structure](#code-structure)
- [Categories](#categories)
- [Response shape (problem+json)](#response-shape-problemjson)
- [Error flow through the lifecycle](#error-flow-through-the-lifecycle)
- [Config errors (PLS-CFG)](#config-errors-pls-cfg)
- [HTTP/proxy errors (PLS-HTTP / PLS-PRX)](#httpproxy-errors-pls-http--pls-prx)
- [TLS/ACME errors (PLS-TLS / PLS-ACME)](#tlsacme-errors-pls-tls--pls-acme)
- [Security/WAF errors (PLS-SEC / PLS-WAF)](#securitywaf-errors-pls-sec--pls-waf)
- [Plugin & admin errors (PLS-PLG / PLS-ADM)](#plugin--admin-errors-pls-plg--pls-adm)
- [Process exit codes](#process-exit-codes)
- [Cross-references](#cross-references)

---

## Why a stable taxonomy

Free-text errors are unsearchable and break scripts when reworded. Pulsate assigns every error a **stable, documented code** so that: operators can search `PLS-PRX-0003` and land on a fix; logs/metrics/alerts key off codes; the API returns machine-actionable problems; and docs are generated from the same registry that defines them (no drift — [17. Documentation](17-documentation.md)). Codes never change meaning; deprecated codes are retired, not reused.

## Code structure

```
PLS-<AREA>-<NNNN>
     │       └ zero-padded number, stable forever
     └ subsystem area
```

Areas: `CFG` (config), `HTTP` (protocol/decode), `PRX` (proxy/upstream), `TLS`, `ACME`, `SEC` (security/auth), `WAF`, `CACHE`, `PLG` (plugin), `ADM` (admin API), `CLU` (cluster), `SYS` (process/runtime).

Each code has: a title, a category, a severity, an HTTP status (if request-facing), an operator remediation, and a docs URL (`https://pulsate.nahsv.com/errors/PLS-...`).

## Categories

| Category | Meaning | Typical surface |
|---|---|---|
| **Client** | the request is bad | 4xx response |
| **Upstream** | a backend failed | 502/503/504 |
| **Config** | invalid configuration | load-time / `p8 validate` / 422 on API |
| **Security** | blocked by policy | 401/403/429 |
| **Internal** | a Pulsate-side fault | 500 + log; never leaks detail to client |
| **Operational** | env/runtime condition | startup failure / exit code / metric |

## Response shape (problem+json)

All request-facing and API errors use RFC 9457:

```json
{ "type": "https://pulsate.nahsv.com/errors/PLS-PRX-0003",
  "title": "Upstream connection failed",
  "status": 502,
  "code": "PLS-PRX-0003",
  "detail": "no healthy target in upstream @api (3 ejected, 1 breaker-open)",
  "instance": "/api/orders",
  "request_id": "01J...ULID",
  "trace_id": "00-2a3f...-..." }
```

- Client-facing error bodies are **safe** — no stack traces, internal addresses, or secrets (Internal-category errors return a generic title + code + request ID; the detail goes only to logs).
- Content negotiation: HTML error pages for browsers (configurable, `on_error`), problem+json for API clients, gRPC status mapping for gRPC.

## Error flow through the lifecycle

Per [07. Middleware](07-middleware.md): any stage returning a `PulsateError` diverts to **[Recover]**, which (1) lets middleware `on_error` handle it, else (2) maps it via this catalog to a response, then (3) resumes [Egress] so response middleware (security headers, CORS, compression, logging) still apply. [Finalize] logs the code + request ID and increments `pulsate_errors_total{code=...}`.

## Config errors (PLS-CFG)

Surfaced at load/`validate`/API; never affect the running snapshot. Examples:

| Code | Title | Remediation |
|---|---|---|
| PLS-CFG-0001 | Syntax error | fix the token at the reported span |
| PLS-CFG-0002 | Unknown directive | check spelling / `flow_version` |
| PLS-CFG-0003 | Type/unit mismatch | provide the expected type (e.g., `30s`) |
| PLS-CFG-0005 | Missing required field | add the field shown |
| PLS-CFG-0007 | Unknown reference (`@name`) | define it or fix the name |
| PLS-CFG-0010 | Host+port collision | two sites bind the same host:port |
| PLS-CFG-0012 | Multiple handlers in one route | a route has exactly one handler |
| PLS-CFG-0015 | ACME challenge unreachable | ensure port 80 / DNS provider configured |
| PLS-CFG-0020 | Duplicate definition (include) | remove the duplicate across `use`d files |

All carry `{file,line,col}` and a fix hint.

## HTTP/proxy errors (PLS-HTTP / PLS-PRX)

| Code | Title | Status |
|---|---|---|
| PLS-HTTP-0001 | Malformed request | 400 |
| PLS-HTTP-0002 | Ambiguous framing (smuggling) | 400 |
| PLS-HTTP-0003 | Header limits exceeded | 431 |
| PLS-HTTP-0004 | Body too large | 413 |
| PLS-HTTP-0005 | Request timeout (header/idle) | 408 |
| PLS-PRX-0001 | No route matched | 404 |
| PLS-PRX-0002 | Upstream connect timeout | 504 |
| PLS-PRX-0003 | No healthy upstream target | 502/503 |
| PLS-PRX-0004 | Upstream response timeout | 504 |
| PLS-PRX-0005 | Circuit breaker open | 503 + Retry-After |
| PLS-PRX-0006 | Retry budget exhausted | 502 |
| PLS-PRX-0007 | Upstream protocol error | 502 |

## TLS/ACME errors (PLS-TLS / PLS-ACME)

| Code | Title | Surface |
|---|---|---|
| PLS-TLS-0001 | No certificate for SNI | handshake alert + log |
| PLS-TLS-0002 | Client cert required/invalid (mTLS) | 403 / handshake alert |
| PLS-TLS-0003 | Protocol/cipher not permitted | handshake alert |
| PLS-ACME-0001 | Challenge failed (HTTP-01/DNS-01) | log + metric + dashboard |
| PLS-ACME-0002 | Rate-limited by CA | log; backoff |
| PLS-ACME-0003 | Renewal failed (cert near expiry) | alert (critical) |
| PLS-ACME-0004 | On-demand host not allow-listed | 403 + log |

## Security/WAF errors (PLS-SEC / PLS-WAF)

| Code | Title | Status |
|---|---|---|
| PLS-SEC-0001 | Authentication required | 401 |
| PLS-SEC-0002 | Token invalid/expired | 401 |
| PLS-SEC-0003 | Authorization denied (claims/cert) | 403 |
| PLS-SEC-0004 | Rate limit exceeded | 429 + Retry-After/RateLimit-* |
| PLS-WAF-0001 | Blocked by WAF rule | 403 |
| PLS-WAF-0002 | Blocked by geo/ASN policy | 403 |
| PLS-WAF-0003 | Bot challenge required/failed | 403/429 |
| PLS-WAF-0004 | IP denied | 403 |

(Security blocks are logged to the audit stream with reason — [09. Security](09-security.md).)

## Plugin & admin errors (PLS-PLG / PLS-ADM)

| Code | Title | Notes |
|---|---|---|
| PLS-PLG-0001 | Plugin load/validation failed | bad/unsigned `.wasm` |
| PLS-PLG-0002 | Plugin trapped | fail-open/closed per policy |
| PLS-PLG-0003 | Plugin exceeded fuel/time | killed; request per policy |
| PLS-PLG-0004 | Capability denied | plugin requested an ungranted capability |
| PLS-PLG-0005 | ABI version unsupported | plugin targets an unsupported world |
| PLS-ADM-0001 | Unauthorized | 401 |
| PLS-ADM-0002 | Forbidden (scope) | 403 |
| PLS-ADM-0003 | Config apply rejected (validation) | 422 (wraps PLS-CFG-*) |
| PLS-ADM-0004 | Conflict (If-Match/optimistic) | 409 |

## Process exit codes

Stable for scripting/CI (mirrors [13. CLI](13-cli.md)):

| Exit | Meaning | Maps to |
|---|---|---|
| 0 | success | — |
| 1 | generic runtime error | PLS-SYS-0001 |
| 2 | config validation failed | PLS-CFG-* |
| 3 | bind failed / port in use | PLS-SYS-0003 |
| 4 | admin API unreachable | PLS-ADM-* |
| 5 | certificate/ACME fatal | PLS-ACME-* |
| 6 | state store error (corrupt/locked) | PLS-SYS-0006 |
| 64 | usage error (bad flags) | — (sysexits convention) |

## Cross-references
- [07. Middleware](07-middleware.md) — the Recover phase mapping errors to responses.
- [22. Admin API](22-admin-api.md) — problem+json on the API.
- [04. Configuration](04-configuration.md) — config diagnostics rendering.
- [15. Observability](15-observability.md) & [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — `pulsate_errors_total{code}` and logging.
- [13. CLI](13-cli.md) — exit codes.
