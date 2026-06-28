# 15. Observability

> Seeing inside Pulsate: metrics, distributed tracing (OpenTelemetry), Prometheus exposition, structured logs, request IDs, and how they correlate so any request can be followed end to end.

**Contents**
- [Principles](#principles)
- [The three pillars & correlation](#the-three-pillars--correlation)
- [Metrics](#metrics)
- [Prometheus](#prometheus)
- [Tracing & OpenTelemetry](#tracing--opentelemetry)
- [Structured logs](#structured-logs)
- [Request IDs & propagation](#request-ids--propagation)
- [Distributed tracing in practice](#distributed-tracing-in-practice)
- [Cross-references](#cross-references)

---

## Principles

- **Observability is built in, not bolted on.** Every [request lifecycle](02-architecture.md#request-lifecycle) stage emits timing and outcome; you don't add a module to see your traffic.
- **Open standards only:** Prometheus exposition and OpenTelemetry (OTLP) — no proprietary agent, no lock-in. Pulsate is a good citizen in any existing stack.
- **Low overhead, bounded cardinality:** instrumentation is cheap (per-core atomics, sampling) and label cardinality is capped by design so metrics can't blow up your TSDB ([26. Metrics Catalog](26-metrics-and-slo-catalog.md)).
- **Correlated by construction:** logs, metrics exemplars, and traces share the request ID and trace ID, so you can pivot between them.

## The three pillars & correlation

```
            request_id = R, trace_id = T (W3C)
   ┌──────────────┬──────────────────┬─────────────────┐
   ▼              ▼                  ▼
 METRICS        TRACES             LOGS
 counters/      span per stage     structured JSON
 histograms     (Accept→Finalize)  lines, each carrying
 w/ exemplars ──┘ exemplar links   {req_id:R, trace_id:T}
   linking a slow histogram bucket to the exact trace,
   and every log line back to the same request.
```

`pulsate-observe` owns all three and the correlation IDs; subsystems just emit through its facades.

## Metrics

- **Facade:** the `metrics` crate facade decouples instrumentation from the exporter; the Prometheus exporter is default, others (OTLP metrics, StatsD) are pluggable.
- **Types:** counters (requests, bytes, errors), gauges (active connections, pool size, breaker state), histograms (latency per stage, body sizes) with configurable buckets.
- **Coverage by subsystem:** listener/TLS (handshakes, ALPN, cert selection), HTTP (by version/method/status), proxy/upstream (latency, retries, health, breaker), cache (hit/miss/SWR), WAF/rate-limit (blocks by reason), plugins (invocations, fuel, traps), control plane (reloads, cert renewals), runtime (task counts, scheduler). The exhaustive list with names, types, labels, and SLO mapping is [26. Metrics Catalog](26-metrics-and-slo-catalog.md).
- **Exemplars:** latency histograms carry exemplars (trace IDs) so a spike in the p99 bucket links straight to a representative trace.

## Prometheus

```
metrics { prometheus { listen 127.0.0.1:9100; path /metrics } }
```

- Standard `/metrics` exposition; naming follows `pulsate_<subsystem>_<name>_<unit>` with base units (seconds, bytes) per Prometheus conventions.
- Ships **recommended recording & alerting rules** and a **Grafana dashboard JSON** (traffic, errors, latency, upstreams, cache, certs) so you get a working dashboard immediately ([26. Metrics Catalog](26-metrics-and-slo-catalog.md)).
- The metrics endpoint binds privately by default; exposing it is explicit. Per-route/site/upstream label dimensions are bounded (high-cardinality labels like raw path are bucketed/templated by route pattern, not literal URL).

## Tracing & OpenTelemetry

- Built on **`tracing`** internally, exported via **OpenTelemetry (OTLP)** to any compatible backend (Jaeger, Tempo, Honeycomb, etc.):
  ```
  tracing { enabled true; exporter otlp; endpoint "http://otel-collector:4317"; sample 0.05; propagate [w3c, b3] }
  ```
- **Spans:** a root server span per request, with child spans per lifecycle stage (Handshake, Match, each middleware, Upstream connect/send/receive, Stream) — so a trace shows exactly where time went, including which middleware was slow.
- **Context propagation:** W3C Trace Context (`traceparent`/`tracestate`) and B3 are parsed from inbound requests and **injected into upstream requests**, so Pulsate is a transparent hop in your distributed trace, not a black box.
- **Sampling:** head-based by default (configurable rate), with support for parent-based sampling (respect upstream sampling decisions) and tail-sampling via the OTel collector.
- **Attributes:** spans carry route, upstream, status, cache result, retry count, and error codes — the same fields as logs/metrics.

## Structured logs

```
log { level info; format json; output stdout
      access { enabled true; fields [ts, method, host, path, status, dur_ms, upstream, cache, req_id, trace_id] }
      sample { success 0.1 } }
```

- **JSON by default** (machine-parseable for Loki/ELK/etc.), with a human `text` mode for local dev.
- **Access logs** are templated, field-selectable, and **sampleable** (sample successes, always log errors) to control volume without losing signal.
- **Error/diagnostic logs** carry the stable error code ([25. Error Catalog](25-error-and-status-catalog.md)), request ID, and context.
- **Audit logs** are a separate, security-focused stream ([09. Security](09-security.md)).
- Outputs: stdout (container-native), file (with rotation), syslog, or OTLP logs. **Secrets are redacted** everywhere.

## Request IDs & propagation

- Every request gets a **request ID** at [Accept]/[Decode]: if the client/upstream sent a trusted `X-Request-ID`/`traceparent`, Pulsate reuses it; otherwise it generates one (ULID-style, sortable).
- The request ID is: added to the response (`X-Request-ID`), forwarded to the upstream, attached to every log line and span for that request, and shown in the dashboard request inspector.
- This is the single thread that ties a user-reported "request at 14:03 failed" to its logs, its trace, and its metrics exemplar — `p8 trace <request-id>` ([13. CLI](13-cli.md)) pulls it all together.

## Distributed tracing in practice

A worked path showing correlation:

```
client ──traceparent: 00-T-...──▶ Pulsate
   [server span T:root  req_id=R]
     ├─ span Match (route=/api/*)            0.04ms
     ├─ span mw:jwt (valid sub=u123)         0.12ms
     ├─ span mw:rate_limit (allowed)         0.01ms
     ├─ span Upstream connect (@api a:8080)  0.30ms (reused pool conn)
     ├─ span Upstream request                12.4ms  ← injects traceparent 00-T-... downstream
     └─ span Egress mw:compress              0.20ms
   log: {ts, method:GET, path:/api/x, status:200, dur_ms:13.1, upstream:"a:8080", cache:MISS, req_id:R, trace_id:T}
   metric: pulsate_http_request_duration_seconds{route="/api/*",status="200"} += 0.0131 (exemplar→T)
```

The downstream service continues trace `T`; Grafana/Jaeger shows Pulsate as a first-class span; a slow `mw:jwt` or `Upstream request` is immediately attributable. This per-stage attribution is also the data foundation for AI-assisted diagnostics ([20. Future](20-future.md)).

## Cross-references
- [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — every metric, label, and SLO.
- [02. Architecture](02-architecture.md) — the lifecycle stages that become spans.
- [11. Dashboard](11-dashboard.md) — live metrics/logs/inspector built on these signals.
- [25. Error Catalog](25-error-and-status-catalog.md) — error codes carried in logs/spans.
- [09. Security](09-security.md) — audit log stream and secret redaction.
