# 22. Admin / Control-Plane API Reference

> The control surface Pulsate exposes for automation: a versioned REST + gRPC API served by the control plane, covering config, runtime state, certificates, cache, upstreams, events, and diagnostics. The CLI and dashboard are clients of this API.

**Contents**
- [Overview & conventions](#overview--conventions)
- [Authentication & authorization](#authentication--authorization)
- [Versioning & stability](#versioning--stability)
- [Errors](#errors)
- [Config endpoints](#config-endpoints)
- [Runtime state endpoints](#runtime-state-endpoints)
- [Certificate endpoints](#certificate-endpoints)
- [Cache endpoints](#cache-endpoints)
- [Upstream & health endpoints](#upstream--health-endpoints)
- [Events & streaming](#events--streaming)
- [Cluster endpoints](#cluster-endpoints)
- [Diagnostics endpoints](#diagnostics-endpoints)
- [Cross-references](#cross-references)

---

## Overview & conventions

- **Base:** `https://<admin-addr>/v1/...` (loopback by default — `pulsate { admin { listen 127.0.0.1:9180 } }`).
- **Two protocols, one model:** REST (JSON, OpenAPI 3 spec) for humans/scripts; gRPC (protobuf, reflection) for high-throughput controllers (e.g., the K8s operator). Both expose the same resources.
- **REST conventions:** resource-oriented nouns; standard verbs (`GET` read, `PUT` replace, `PATCH` modify, `POST` action, `DELETE` remove); JSON bodies; `?page=&limit=` cursor pagination for lists; `If-Match`/ETag for optimistic concurrency on config.
- **Idempotency:** mutating POST actions accept an `Idempotency-Key` header so retries are safe.
- **Everything is also a CLI command** ([13. CLI](13-cli.md)) and a dashboard action ([11. Dashboard](11-dashboard.md)) — unified surface.

## Authentication & authorization

- **AuthN:** bearer tokens (issued/managed via config or the API), mTLS client certs, or OIDC/SSO. On loopback, a short-lived local token (`pulsate dashboard open`) bootstraps access.
- **AuthZ (RBAC):** every endpoint requires a scope; tokens carry roles mapping to scopes:

| Role | Scopes (examples) |
|---|---|
| `viewer` | `config:read`, `state:read`, `metrics:read`, `cert:read`, `cache:read` |
| `operator` | viewer + `cache:purge`, `cert:renew`, `config:reload`, `upstream:drain` |
| `admin` | operator + `config:write`, `cert:write`, `token:manage`, `cluster:manage` |

- **Audit:** every mutating call is recorded (actor, action, diff, timestamp) in the tamper-evident audit log ([09. Security](09-security.md)).
- **Hardening:** auth-rate-limited; CSRF protection for browser (dashboard) calls; exposing the admin listener publicly requires explicit config + configured auth, with a startup warning otherwise ([21. Threat Model](21-threat-model.md)).

## Versioning & stability

- Path-versioned (`/v1`). Within a major, only **additive** changes (new fields/endpoints); breaking changes bump the major with an overlap window.
- The OpenAPI/proto specs are the source of truth and generate the reference docs ([17. Documentation](17-documentation.md)) and client stubs.

## Errors

All errors use **RFC 9457 `application/problem+json`** with a stable Pulsate error code ([25. Error Catalog](25-error-and-status-catalog.md)):

```json
{ "type": "https://squaretick.dev/pulsate/errors/PLS-CFG-0007",
  "title": "Unknown upstream reference",
  "status": 422,
  "code": "PLS-CFG-0007",
  "detail": "route /api/* references undefined upstream @apii",
  "instance": "/v1/config",
  "location": { "file": "pulsate.flow", "line": 14, "col": 31 } }
```

## Config endpoints

| Method & path | Scope | Description |
|---|---|---|
| `GET /v1/config` | `config:read` | current config (source + effective, secrets redacted) + snapshot hash |
| `GET /v1/config/effective` | `config:read` | fully-resolved effective config |
| `POST /v1/config/validate` | `config:read` | validate a candidate config; returns errors/warnings, no apply |
| `POST /v1/config/diff` | `config:read` | diff candidate vs current at the snapshot level |
| `PUT /v1/config` | `config:write` | replace config → validate → atomic reload (supports `dry_run`, `If-Match`) |
| `POST /v1/config/reload` | `config:reload` | reload from the configured source |
| `POST /v1/config/rollback` | `config:write` | roll back to the previous snapshot generation |
| `GET /v1/config/history` | `config:read` | applied-change history (actor, hash, diff) |

`PUT /v1/config` example:
```bash
curl -X PUT https://127.0.0.1:9180/v1/config \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: text/x-pulsate-flow" \
  --data-binary @pulsate.flow            # ?dry_run=true to preview only
```

## Runtime state endpoints

| Method & path | Scope | Description |
|---|---|---|
| `GET /v1/status` | `state:read` | uptime, version, snapshot hash, readiness, worker health |
| `GET /v1/listeners` | `state:read` | bound sockets, protocols, connection counts |
| `GET /v1/routes` | `state:read` | compiled routing table in precedence order |
| `POST /v1/routes/explain` | `state:read` | `{host,path,method}` → matched site/route/middleware/handler |
| `GET /v1/metrics` | `metrics:read` | Prometheus exposition (also on the metrics listener) |

## Certificate endpoints

| Method & path | Scope | Description |
|---|---|---|
| `GET /v1/certs` | `cert:read` | inventory (hosts, issuer, expiry, source, renewal status) |
| `GET /v1/certs/{host}` | `cert:read` | chain, validity, OCSP, fingerprints |
| `POST /v1/certs/{host}/renew` | `cert:renew` | trigger renewal (`?force=true`) |
| `PUT /v1/certs/{host}` | `cert:write` | install a manual cert/key |
| `GET /v1/acme/challenges` | `cert:read` | recent ACME challenge attempts/errors |

## Cache endpoints

| Method & path | Scope | Description |
|---|---|---|
| `GET /v1/cache/stats` | `cache:read` | hit ratio, size, evictions, bytes saved (per cache) |
| `POST /v1/cache/purge` | `cache:purge` | body `{tags?,urls?,prefixes?,all?,soft?}`; propagates cluster-wide |
| `GET /v1/cache/entries/{key}` | `cache:read` | entry metadata (validators, age, vary, tags) |
| `POST /v1/cache/warm` | `cache:purge` | pre-populate from a URL list |

## Upstream & health endpoints

| Method & path | Scope | Description |
|---|---|---|
| `GET /v1/upstreams` | `state:read` | pools, targets, weights, health, breaker state, in-flight |
| `GET /v1/upstreams/{name}` | `state:read` | detail incl. per-target latency (EWMA) |
| `POST /v1/upstreams/{name}/targets/{id}/drain` | `upstream:drain` | drain/eject a target |
| `POST /v1/upstreams/{name}/targets/{id}/health` | `operator` | force healthy/unhealthy (maintenance) |

## Events & streaming

| Path | Transport | Description |
|---|---|---|
| `GET /v1/events` | **SSE** | live event stream: reload done, cert issued/renewed, upstream up/down, breaker opened, WAF block bursts |
| `GET /v1/logs` | **SSE** | filtered live access/error logs (bounded/sampled) |
| `GET /v1/inspect` | **WebSocket** | live request-inspector tap with a filter; per-request lifecycle attribution ([11. Dashboard](11-dashboard.md)) |
| `GET /v1/metrics/stream` | **SSE** | periodic metric snapshots for live dashboards |

These power the dashboard's live views without polling.

## Cluster endpoints

| Method & path | Scope | Description |
|---|---|---|
| `GET /v1/cluster` | `state:read` | members, roles (leader/peer), health, versions |
| `GET /v1/cluster/leader` | `state:read` | current leader (cert-issuance owner) |
| `POST /v1/cluster/drain` | `cluster:manage` | drain this node for maintenance |

## Diagnostics endpoints

| Method & path | Scope | Description |
|---|---|---|
| `GET /v1/debug/pprof/{profile}` | `admin` (loopback) | CPU/heap/goroutine-equivalent profiles ([10. Performance](10-performance.md)) |
| `GET /v1/debug/config-snapshot` | `admin` | the raw compiled snapshot (debugging) |
| `GET /v1/healthz` / `GET /v1/readyz` | none/limited | liveness/readiness probes (for orchestrators) |
| `GET /v1/version` | none | version, `flow_version` range, plugin ABI range |

`grpcurl` example:
```bash
grpcurl -H "authorization: Bearer $TOKEN" 127.0.0.1:9180 pulsate.v1.Admin/GetStatus
```

## Cross-references
- [11. Dashboard](11-dashboard.md) — the primary client of this API.
- [13. CLI](13-cli.md) — the CLI maps 1:1 to these endpoints.
- [09. Security](09-security.md) & [21. Threat Model](21-threat-model.md) — admin-surface authz & hardening.
- [25. Error Catalog](25-error-and-status-catalog.md) — the problem+json codes returned.
- [02. Architecture](02-architecture.md) — internal API tiers and the snapshot the API mutates.
