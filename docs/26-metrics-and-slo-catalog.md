# 26. Metrics and SLO Catalog

> Every metric Pulsate emits — name, type, unit, labels, and meaning — grouped by subsystem, plus the SLIs/SLOs they feed, recommended Prometheus rules, and a Grafana dashboard layout. Cardinality is bounded by design.

**Contents**
- [Conventions & cardinality](#conventions--cardinality)
- [Listener & TLS metrics](#listener--tls-metrics)
- [HTTP metrics](#http-metrics)
- [Proxy & upstream metrics](#proxy--upstream-metrics)
- [Cache metrics](#cache-metrics)
- [Security (WAF/rate-limit) metrics](#security-wafrate-limit-metrics)
- [Plugin metrics](#plugin-metrics)
- [Control-plane & runtime metrics](#control-plane--runtime-metrics)
- [SLIs & SLOs](#slis--slos)
- [Recommended Prometheus rules](#recommended-prometheus-rules)
- [Grafana dashboard layout](#grafana-dashboard-layout)
- [Cross-references](#cross-references)

---

## Conventions & cardinality

- **Naming:** `pulsate_<subsystem>_<name>_<unit>` with Prometheus base units (`_seconds`, `_bytes`, `_total`). Histograms expose `_bucket`/`_sum`/`_count` and carry **trace exemplars** ([15. Observability](15-observability.md)).
- **Cardinality is capped:** labels use **route patterns** (`/api/*`), not literal URLs; `site`, `upstream`, `code`, `status_class` are bounded sets. High-cardinality dimensions (client IP, full path, user) are **never** metric labels — they live in logs/traces. A built-in cardinality guard drops/aggregates labels that exceed a configured budget and warns.
- **Common labels:** `site`, `route`, `upstream`, `target`, `proto` (`h1|h2|h3`), `status`, `status_class` (`2xx`…`5xx`), `code` (PLS-* for errors), `cache` (name).

## Listener & TLS metrics

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `pulsate_listener_connections_active` | gauge | `listener`,`proto` | current connections |
| `pulsate_listener_connections_total` | counter | `listener`,`proto` | accepted connections |
| `pulsate_listener_accept_errors_total` | counter | `listener`,`reason` | accept failures / limit rejections |
| `pulsate_tls_handshakes_total` | counter | `result`(`ok|fail`),`version`,`proto` | TLS/QUIC handshakes |
| `pulsate_tls_handshake_duration_seconds` | histogram | `version` | handshake latency |
| `pulsate_tls_cert_selected_total` | counter | `sni_matched`(`exact|wildcard|default`) | SNI selection outcomes |
| `pulsate_tls_session_resumptions_total` | counter | `kind`(`ticket|cache`) | resumption rate |

## HTTP metrics

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `pulsate_http_requests_total` | counter | `site`,`route`,`proto`,`method`,`status` | request count |
| `pulsate_http_request_duration_seconds` | histogram | `site`,`route`,`status_class` | end-to-end latency (the core SLI) |
| `pulsate_http_request_bytes` / `pulsate_http_response_bytes` | histogram | `route` | payload sizes |
| `pulsate_http_inflight` | gauge | `route` | concurrent in-flight requests |
| `pulsate_http_active_streams` | gauge | `proto` | h2/h3 concurrent streams |
| `pulsate_errors_total` | counter | `code`,`category`,`status_class` | errors by `PLS-*` code ([25](25-error-and-status-catalog.md)) |
| `pulsate_http2_resets_total` | counter | `reason` | rapid-reset/abuse signal |

## Proxy & upstream metrics

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `pulsate_upstream_requests_total` | counter | `upstream`,`target`,`status` | proxied requests |
| `pulsate_upstream_request_duration_seconds` | histogram | `upstream`,`target` | upstream latency ([Upstream] stage) |
| `pulsate_upstream_connect_duration_seconds` | histogram | `upstream` | connect/pool-acquire time |
| `pulsate_upstream_pool_connections` | gauge | `upstream`,`state`(`idle|active`) | pool occupancy |
| `pulsate_upstream_healthy_targets` | gauge | `upstream` | healthy target count |
| `pulsate_upstream_retries_total` | counter | `upstream`,`reason` | retries (watch for storms) |
| `pulsate_upstream_breaker_state` | gauge | `upstream`,`target` | 0=closed,1=half,2=open |
| `pulsate_upstream_ejections_total` | counter | `upstream`,`target` | passive-health ejections |

## Cache metrics

(See [08. Cache](08-cache.md).)

| Metric | Type | Labels |
|---|---|---|
| `pulsate_cache_requests_total` | counter | `cache`,`route`,`result`(`hit|miss|stale|revalidated|bypass`) |
| `pulsate_cache_bytes` | gauge | `cache`,`store`(`memory|disk|redis`) |
| `pulsate_cache_entries` | gauge | `cache` |
| `pulsate_cache_evictions_total` | counter | `cache`,`reason` |
| `pulsate_cache_revalidations_total` | counter | `cache`,`outcome`(`304|200|error`) |
| `pulsate_cache_origin_saved_bytes_total` | counter | `cache` |
| `pulsate_cache_swr_served_total` / `pulsate_cache_stale_if_error_served_total` | counter | `cache` |

## Security (WAF/rate-limit) metrics

| Metric | Type | Labels |
|---|---|---|
| `pulsate_waf_evaluations_total` | counter | `ruleset`,`action`(`allow|block|detect|challenge`) |
| `pulsate_waf_blocked_total` | counter | `reason`(`rule|geo|asn|bot|ip`),`rule_id` |
| `pulsate_ratelimit_decisions_total` | counter | `limiter`,`decision`(`allow|throttle`) |
| `pulsate_auth_failures_total` | counter | `scheme`(`jwt|basic|mtls|forward`),`reason` |
| `pulsate_audit_records_total` | counter | `kind` |

## Plugin metrics

| Metric | Type | Labels |
|---|---|---|
| `pulsate_plugin_invocations_total` | counter | `plugin`,`world`,`result`(`ok|trap|denied`) |
| `pulsate_plugin_duration_seconds` | histogram | `plugin` |
| `pulsate_plugin_fuel_used` | histogram | `plugin` |
| `pulsate_plugin_instances` | gauge | `plugin`,`state`(`idle|busy`) |

## Control-plane & runtime metrics

| Metric | Type | Labels |
|---|---|---|
| `pulsate_config_reloads_total` | counter | `result`(`ok|rejected|rolled_back`) |
| `pulsate_config_snapshot_generation` | gauge | — |
| `pulsate_config_last_reload_timestamp_seconds` | gauge | — |
| `pulsate_cert_expiry_seconds` | gauge | `host` (→ alert when low) |
| `pulsate_cert_renewals_total` | counter | `result` |
| `pulsate_cluster_members` | gauge | `state`(`alive|suspect|dead`) |
| `pulsate_runtime_tasks` | gauge | `kind` |
| `pulsate_runtime_worker_busy_ratio` | gauge | `worker` |
| `pulsate_build_info` | gauge=1 | `version`,`commit`,`rustc` |
| `pulsate_process_*` (RSS, CPU, fds, open files) | gauge/counter | — |

## SLIs & SLOs

Recommended starting SLOs (operators tune):

| SLI | Definition | Example SLO |
|---|---|---|
| Availability | non-5xx (excluding upstream-origin 5xx) / total | ≥ 99.95% |
| Latency (proxy overhead) | Pulsate-added latency p99 (`request_duration` − `upstream_duration`) | p99 ≤ a few ms |
| Edge latency | `pulsate_http_request_duration_seconds` p99 by route | per-route target |
| TLS success | `tls_handshakes_total{result=ok}` / total | ≥ 99.9% |
| Cache effectiveness | hit ratio for cacheable routes | route-specific target |
| Cert health | min `pulsate_cert_expiry_seconds` | always > 14 days |

Error budgets derive from these; latency uses the **Pulsate-overhead** SLI so a slow origin doesn't burn Pulsate's budget.

## Recommended Prometheus rules

Shipped as a rules file (illustrative):

```yaml
groups:
- name: p8.rules
  rules:
  - record: p8:http_error_ratio:5m
    expr: sum(rate(pulsate_http_requests_total{status_class="5xx"}[5m])) by (site)
        / sum(rate(pulsate_http_requests_total[5m])) by (site)
  - alert: PulsateHighErrorRate
    expr: p8:http_error_ratio:5m > 0.02
    for: 10m
    labels: { severity: page }
  - alert: PulsateCertExpiringSoon
    expr: min(pulsate_cert_expiry_seconds) by (host) < 14*24*3600
    for: 1h
    labels: { severity: ticket }
  - alert: PulsateBreakerOpen
    expr: max(pulsate_upstream_breaker_state) by (upstream) == 2
    for: 5m
    labels: { severity: page }
  - alert: PulsateReloadRejected
    expr: increase(pulsate_config_reloads_total{result="rejected"}[15m]) > 0
    labels: { severity: ticket }
```

## Grafana dashboard layout

The shipped dashboard JSON ([15. Observability](15-observability.md)) is organized top-down:
1. **Golden signals row:** rps, error ratio, latency p50/p99/p999, saturation (inflight, busy-ratio).
2. **Traffic:** by site/route/proto/status; top routes; request/response sizes.
3. **Upstreams:** latency, healthy targets, retries, breaker states, ejections.
4. **Cache:** hit ratio, bytes saved, evictions, SWR/SIE serves.
5. **Security:** WAF blocks by reason, rate-limit throttles, auth failures.
6. **Certs & control plane:** expiry countdown, renewals, reload outcomes, cluster members.
7. **Runtime:** CPU/RSS/fds, task counts, worker busy-ratio.

Exemplar links jump from a latency spike to the exact trace.

## Cross-references
- [15. Observability](15-observability.md) — exporters, exemplars, correlation.
- [08. Cache](08-cache.md), [06. Reverse Proxy](06-reverse-proxy.md), [09. Security](09-security.md), [12. Plugins](12-plugins.md) — subsystem metrics in context.
- [25. Error Catalog](25-error-and-status-catalog.md) — `pulsate_errors_total{code}`.
- [11. Dashboard](11-dashboard.md) — built-in views over these metrics.
