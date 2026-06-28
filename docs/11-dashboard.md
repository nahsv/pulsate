# 11. Dashboard

> The built-in web dashboard: an embedded, zero-dependency operator UI for observing and (optionally) editing a running Pulsate — its architecture, backend, frontend, authentication, and the live views (metrics, logs, config editor, certificate manager, request inspector, cache stats).

**Contents**
- [Philosophy & architecture](#philosophy--architecture)
- [Backend](#backend)
- [Frontend](#frontend)
- [Authentication & authorization](#authentication--authorization)
- [Metrics views](#metrics-views)
- [Live logs](#live-logs)
- [Configuration editor](#configuration-editor)
- [Certificate manager](#certificate-manager)
- [Request inspector](#request-inspector)
- [Cache statistics](#cache-statistics)
- [Cross-references](#cross-references)

---

## Philosophy & architecture

The dashboard keeps the **one-binary** promise: it is a static single-page app **compiled into the Pulsate binary** (`pulsate-dashboard` embeds the built assets via `rust-embed`), served by the control plane. There is no separate Node service, no external process, nothing to deploy. It is **off by default** and, when enabled, binds to **loopback** unless explicitly exposed (secure by default).

```
browser ──HTTPS──▶ pulsate-control (admin listener)
                     ├── GET /            → embedded Svelte SPA (pulsate-dashboard)
                     ├── /v1/...          → Admin API (REST)        [22. Admin API]
                     ├── /v1/events (SSE) → live metrics/logs/events stream
                     └── /v1/ws           → bidirectional (request inspector taps)
```

The dashboard is a **pure client of the Admin API** — it can do nothing the API can't, which means everything it offers is also scriptable and the security model is unified ([22. Admin API](22-admin-api.md)).

## Backend

- Served by `pulsate-control` on the admin listener (`pulsate { admin { listen 127.0.0.1:9180 } }`), separate from the data-plane traffic ports.
- **Data sources:** the admin API exposes config, runtime state (routes, upstreams, health, breakers), certificates, cache stats, and metrics; an **SSE** endpoint streams live updates (metrics ticks, log lines, events like "cert renewed", "upstream down"); a **WebSocket** endpoint powers the request inspector's live tap.
- **No new state:** the dashboard backend introduces no datastore of its own; it reads the same control-plane state and metrics registry the rest of Pulsate uses.
- **Resource-safe:** log/inspector streams are bounded and sampled so opening the dashboard can never degrade serving.

## Frontend

- A **Svelte** SPA (chosen for tiny bundle size and no runtime framework overhead — fits "one binary" ethos), built at compile time and embedded. Loads fast, works offline against the local API, and is fully usable over a slow SSH-tunneled connection.
- **Information-dense but legible:** an overview (traffic, error rate, p99, active certs, cache hit ratio), then drill-downs per site/route/upstream.
- **Read-only by default; editing is gated** (see auth). The UI clearly indicates when it is in a read-only vs. read-write session.
- Accessible (keyboard-navigable, ARIA), dark/light, and themeable.

## Authentication & authorization

- **Local-first:** on loopback, the dashboard can use a one-time CLI-issued token (`p8 dashboard open` prints a localhost URL with a short-lived token) — zero config to start.
- **Exposed deployments** require explicit auth: built-in username/password (hashed, secret-stored), OIDC/SSO (delegate to your IdP), or mTLS client certs. Admin API tokens scope access.
- **RBAC:** roles (viewer, operator, admin) map to admin-API scopes — a viewer sees metrics/logs; an operator can purge cache / reload; an admin can edit config and manage certs ([29. Multi-Tenancy](29-multi-tenancy-and-isolation.md)). All privileged actions are **audit-logged** ([09. Security](09-security.md)).
- **Secure by default:** never auto-exposed; binding it to a public interface requires `admin { listen 0.0.0.0:... }` *and* configured auth, with a loud startup warning if auth is missing.

## Metrics views

- **Live overview:** requests/sec, error rate, latency percentiles (p50/p90/p99/p999), bytes in/out, active connections — globally and per site/route/upstream, streamed via SSE.
- **Upstream health:** per-target health, in-flight, latency (EWMA), breaker state (closed/open/half-open), ejections — the operational heartbeat of the proxy.
- **Backed by the same metrics** exported to Prometheus ([26. Metrics Catalog](26-metrics-and-slo-catalog.md)); the dashboard is a convenience, not a replacement for your TSDB. Links out to Grafana where configured.

## Live logs

- **Streaming access/error logs** with server-side filtering (by status, route, upstream, IP, request ID) so you can tail production safely without grepping files.
- **Structured:** each line expands to its JSON fields; click a request ID to jump to the request inspector / trace.
- **Bounded & sampled:** the stream is rate-limited and sampled under high volume to protect the data plane; full logs still go to your configured sink.

## Configuration editor

- **View and (optionally) edit** `pulsate.flow` with syntax highlighting for the Flow language, inline validation (the same `p8 validate` engine, surfacing `PLS-CFG-*` errors with spans), and a **diff preview** of the resulting `ConfigSnapshot` change.
- **Safe apply:** editing goes through validate → diff → confirm → atomic reload, with the auto-rollback guard window ([02. Architecture](02-architecture.md#hot-reload-architecture)). You see exactly what will change before it does.
- **Version history:** every applied change is recorded (actor, diff, hash) in the audit log; you can view past versions and one-click **roll back**.
- **GitOps-friendly:** in environments where config is managed in Git/CRD, the editor is read-only and points you to the source of truth (no split-brain).

## Certificate manager

- **Inventory:** every certificate — hostnames, issuer, validity window, days-to-expiry, source (ACME/manual), and renewal status — with at-a-glance expiry warnings.
- **Actions (operator/admin):** trigger issuance/renewal, view ACME challenge status and recent errors, upload a manual cert, and inspect the chain/OCSP status.
- **Alerts:** near-expiry and renewal-failure surface as dashboard banners and metrics/alerts ([26. Metrics Catalog](26-metrics-and-slo-catalog.md)).

## Request inspector

The standout developer-experience feature: a **live, opt-in tap** on real requests for debugging.
- Start a tap with a filter (host/path/status/header); matching requests stream to the inspector showing the **full [request lifecycle](02-architecture.md#request-lifecycle)**: which site/route matched, each middleware step and its decision (e.g., "rate_limit: allowed", "jwt: valid sub=…", "cache: MISS"), the upstream chosen, per-stage timings, and the final response.
- **Replay/curl export:** turn a captured request into a `curl` command or replay it (in dev mode) to reproduce a bug.
- **Privacy-aware:** sensitive headers/bodies are redacted per policy; taps are bounded, time-limited, and audit-logged. This is the GUI counterpart to `p8 inspect` ([13. CLI](13-cli.md)).

## Cache statistics

- **Effectiveness:** hit ratio (overall, per cache, per route), bytes served from cache, origin bytes saved, eviction rate, and SWR/stale-if-error serve counts.
- **Top objects:** most-requested keys, largest objects, soon-to-expire hot keys.
- **Invalidation:** issue a purge (by tag/URL/prefix/all) from the UI (operator+), see propagation across cluster nodes, and review recent invalidation activity.
- All sourced from the cache metrics in [08. Cache](08-cache.md) / [26. Metrics Catalog](26-metrics-and-slo-catalog.md).

## Cross-references
- [22. Admin API](22-admin-api.md) — the API the dashboard consumes; the real contract.
- [15. Observability](15-observability.md) & [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — metrics behind the views.
- [09. Security](09-security.md) — auth, RBAC, audit logging, secret redaction.
- [13. CLI](13-cli.md) — `p8 inspect`, `p8 dashboard`, the CLI equivalents.
- [08. Cache](08-cache.md) — cache stats and purge semantics.
