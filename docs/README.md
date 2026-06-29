# Pulsate — Design & Documentation Overview

> **Pulsate** is a brand-new, original open-source reverse proxy and application gateway written in Rust: one static binary, one config file, one command — secure by default, batteries included, production-ready, extensible, and enterprise-capable. This document is the entry point to the complete implementation plan; the plan is split across 33 focused documents under `docs/`, linked below.
>
> *Not a fork or clone of Caddy, Nginx, Envoy, or Traefik — it learns from all of them and copies none.*

---

## What Pulsate is

The reverse-proxy market forces a choice between *developer-friendly but you outgrow it* (Caddy) and *infinitely capable but operationally heavy* (Nginx-as-ingress, Envoy, Traefik). Pulsate refuses that trade-off. It rebuilds the gateway from first principles in memory-safe Rust around a single coherent idea: **a configuration that reads the way a request flows**, compiled into an immutable snapshot that a lock-free data plane serves while an in-process control plane manages TLS, config, and policy.

The result is a gateway a solo developer runs in 30 seconds (`pulsate up` → automatic HTTPS) whose *same config shape* scales to a multi-tenant, multi-region enterprise fleet — without ever switching tools.

The ten design pillars: **one binary · one config · one command · secure by default · batteries included · extremely easy to use · production-ready · extensible · cloud-native · enterprise-capable.** See [01. Vision](01-vision.md).

## Quick facts

| Property | Decision |
|---|---|
| Name / binary | **Pulsate** / `pulsate` |
| Language / edition / MSRV | Rust · edition 2021 · MSRV = latest stable − 2 |
| Async runtime | Tokio, behind a `pulsate-rt` abstraction (future io_uring/thread-per-core backend) |
| HTTP stack | hyper (HTTP/1.1 + HTTP/2) · quinn + h3 (HTTP/3/QUIC) |
| TLS | rustls (no OpenSSL) · automatic ACME via instant-acme |
| Config format | **Pulsate Flow** — an original DSL, file extension `.flow`, with the `~>` flow operator |
| State store | redb (embedded, pure-Rust) · optional Redis for shared cluster state |
| Plugins | WebAssembly (Wasmtime, Component Model, WIT), capability-sandboxed |
| Dashboard | Svelte SPA embedded in the binary (`rust-embed`) |
| Telemetry | Prometheus + OpenTelemetry (OTLP) |
| License | Apache-2.0 (open core) + a future separately-licensed enterprise edition |
| Workspace | ~24 `pulsate-*` crates in one Cargo workspace |
| Architecture | in-process control plane / data plane split via an atomically-swapped `ConfigSnapshot` |

## How to read this plan

The plan is **34 documents**: 20 core documents (`01`–`20`) covering vision through future, and 13 deep-dive references (`21`–`33`) that a real build needs. Recommended reading orders:

- **Executives / evaluators:** [01. Vision](01-vision.md) → [19. Milestones](19-milestones.md) → [20. Future](20-future.md).
- **Architects:** [01](01-vision.md) → [02. Architecture](02-architecture.md) → [04. Configuration](04-configuration.md) → [24. ADRs](24-architecture-decision-records.md) → [21. Threat Model](21-threat-model.md).
- **Engineers about to build:** [02](02-architecture.md) → [03. Repository](03-repository.md) → your subsystem (`05`–`12`) → [25. Errors](25-error-and-status-catalog.md)/[26. Metrics](26-metrics-and-slo-catalog.md)/[28. Testing](28-testing-and-conformance.md) → [19. Milestones](19-milestones.md).
- **Operators:** [04. Configuration](04-configuration.md) → [16. Deployment](16-deployment.md) → [15. Observability](15-observability.md) → [32. DR/HA](32-disaster-recovery-and-ha.md) → [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md).

## Table of contents

### Core (01–20)

| # | Document | In one line |
|---|---|---|
| 01 | [Vision](01-vision.md) | Mission, philosophy, target users, non-goals, incumbent comparison, 5-year roadmap. |
| 02 | [Architecture](02-architecture.md) | Control/data-plane split, request lifecycle, snapshot model, hot reload, runtime, DI. |
| 03 | [Repository & Engineering](03-repository.md) | The Cargo workspace, every crate, dependency graph, standards, testing, CI/CD, releases. |
| 04 | [Configuration](04-configuration.md) | Pulsate Flow — the original config language, full spec, many examples. |
| 05 | [HTTP Stack](05-http-stack.md) | HTTP/1.1/2/3, TLS, QUIC, WebSocket, SSE, gRPC, streaming, pooling, zero-copy. |
| 06 | [Reverse Proxy](06-reverse-proxy.md) | Routing, rewriting, LB, sticky, retries, breakers, health, discovery. |
| 07 | [Middleware](07-middleware.md) | The `~>` pipeline: Ingress/Egress/Recover, ordering, the middleware contract. |
| 08 | [Cache](08-cache.md) | Memory/disk/Redis, RFC-9111 semantics, SWR, tags, ranges, compression-aware. |
| 09 | [Security](09-security.md) | TLS, certs, WAF, rate limiting, bot/geo/ASN, JWT, mTLS, secrets, audit. |
| 10 | [Performance](10-performance.md) | Goals, benchmark methodology, lock-free design, allocation, SIMD, kernel offloads. |
| 11 | [Dashboard](11-dashboard.md) | Embedded UI: metrics, live logs, config editor, cert manager, request inspector. |
| 12 | [Plugins](12-plugins.md) | WASM extension model, sandboxing, WIT ABI, versioning, SDK, marketplace. |
| 13 | [CLI](13-cli.md) | Every `pulsate` command, flags, validation, diagnostics, benchmark, migrate, upgrade. |
| 14 | [Developer Experience](14-developer-experience.md) | Install, init, app detection, Rails/Node/Go/Rust, Docker, K8s, dev/debug modes. |
| 15 | [Observability](15-observability.md) | Metrics, tracing, OTel, Prometheus, structured logs, request IDs, correlation. |
| 16 | [Deployment](16-deployment.md) | Bare metal, systemd, Docker, Compose, Kubernetes, cloud, multi-node, cluster. |
| 17 | [Documentation](17-documentation.md) | Diátaxis structure, tutorials, reference (generated), contribution, API docs. |
| 18 | [Open Source](18-open-source.md) | License, governance, CoC, contribution, RFC process, security policy. |
| 19 | [Milestones](19-milestones.md) | Phased plan: objectives, LOC, duration, deliverables, tests, risks, acceptance. |
| 20 | [Future](20-future.md) | Enterprise edition, cloud control plane, managed service, AI diagnostics, multi-region. |

### Deep-dive references (21–33)

| # | Document | In one line |
|---|---|---|
| 21 | [Threat Model](21-threat-model.md) | STRIDE analysis, trust boundaries, abuse cases, threat→mitigation matrix. |
| 22 | [Admin / Control-Plane API](22-admin-api.md) | The REST+gRPC control surface the CLI and dashboard consume. |
| 23 | [Data & State Model](23-data-and-state-model.md) | In-memory and persistent state, on-disk layout, encryption, shared state. |
| 24 | [Architecture Decision Records](24-architecture-decision-records.md) | 20 ADRs recording the *why* of every major choice. |
| 25 | [Error & Status Catalog](25-error-and-status-catalog.md) | Stable `PLS-*` codes, problem+json, exit codes, the Recover flow. |
| 26 | [Metrics & SLO Catalog](26-metrics-and-slo-catalog.md) | Every metric, label, SLI/SLO, Prometheus rules, Grafana layout. |
| 27 | [Configuration Reference](27-config-reference.md) | Exhaustive key-by-key reference for the Flow format. |
| 28 | [Testing & Conformance](28-testing-and-conformance.md) | Unit/property/fuzz/conformance/load/soak/chaos and release gates. |
| 29 | [Multi-Tenancy & Isolation](29-multi-tenancy-and-isolation.md) | Tenant model, namespacing, quotas, RBAC, blast-radius containment. |
| 30 | [Migration & Import](30-migration-and-import.md) | `pulsate import` from nginx/Caddy/HAProxy/Apache with fidelity reporting. |
| 31 | [Benchmarking & Tuning](31-benchmarking-and-tuning.md) | Reproducible benchmark matrix and the operator tuning guide. |
| 32 | [Disaster Recovery & HA](32-disaster-recovery-and-ha.md) | Redundancy, backup/restore, RPO/RTO, split-brain, runbooks. |
| 33 | [Release Engineering & Supply Chain](33-release-engineering-and-supply-chain.md) | Reproducible builds, SBOM, signing/SLSA, channels, update verification. |

## Document synopses

**01 Vision** — Establishes the thesis (collapse the developer-friendly vs. infrastructure-grade divide), the ten pillars as constraints, three concentric audiences (solo dev → platform team → enterprise), explicit non-goals (not a forward proxy, not a full mesh, not a templating engine), an honest comparison table, and a five-year arc.

**02 Architecture** — The spine. Defines the in-process control/data-plane split joined only by an immutable `ConfigSnapshot` published via `arc-swap`; the ten named request-lifecycle stages (Accept→…→Finalize, plus Recover); process/thread/runtime/memory models; snapshot-swap hot reload with resource carry-over; graceful shutdown; trait-based DI; and the extension seams.

**03 Repository & Engineering** — The ~24-crate Cargo workspace with a layered DAG (data plane never depends on control plane), every crate's purpose, coding standards (`forbid(unsafe)` by default, no panics on request paths), the testing pyramid, CI/CD, release channels, and independent versioning of binary/`flow_version`/plugin-ABI.

**04 Configuration** — Pulsate Flow: the original, typed, declarative DSL whose signature is the `~>` flow operator (a route reads as match → middleware → handler). Full grammar, value types, every block, and many complete worked examples from a 5-line start to a multi-tenant fleet.

**05 HTTP Stack** — Protocol mechanics across H1/H2/H3, rustls TLS, ACME, QUIC, WebSocket, SSE, gRPC/gRPC-Web, end-to-end streaming with backpressure, connection pooling/keep-alive, the full timeout ladder, buffer pooling, and zero-copy (sendfile/kTLS/io_uring) opportunities.

**06 Reverse Proxy** — The compiled routing table and deterministic precedence; host/path/regex/predicate routing; header rewriting; weighted/canary routing; sticky sessions; LB policies; idempotency-aware retries with budgets; circuit breakers; active+passive health checks; and dynamic service discovery.

**07 Middleware** — The flat-driver onion pipeline: Ingress (declared order) → Dispatch → Egress (reverse) → Recover, the single `Middleware` contract that unifies built-in/plugin/native middleware, short-circuiting, and composition rules.

**08 Cache** — Correct-by-default HTTP caching: pluggable stores, key/Vary composition, RFC-9111 freshness, ETag/Last-Modified validators, range requests, stale-while-revalidate/stale-if-error, single-flight background refresh, tag-based invalidation, compression-aware caching, and metrics.

**09 Security** — The secure-by-default posture and every control: TLS hardening, full ACME cert lifecycle, the WAF, rate limiting, bot/geo/ASN, JWT/mTLS auth, default security headers, secrets backends, and tamper-evident audit logging.

**10 Performance** — Performance as a measured property: throughput/latency(tail)/memory/scalability goals, a reproducible and honest benchmark methodology with CI regression gating, lock-free structures, allocation strategy, SIMD, and kernel optimizations.

**11 Dashboard** — The embedded Svelte operator UI (a pure Admin-API client): live metrics, filtered live logs, the safe config editor (validate→diff→apply→rollback), certificate manager, the lifecycle-attributing request inspector, and cache stats.

**12 Plugins** — Why WebAssembly, the Wasmtime/Component-Model/WIT host model, capability-based sandboxing with fuel/epoch limits, independent ABI versioning, the SDK, and OCI-based signed distribution + marketplace.

**13 CLI** — The complete `pulsate` command reference (lifecycle, config, certs, cache, diagnostics like `inspect`/`doctor`/`explain`, benchmark, plugins, import, upgrade) and stable exit codes.

**14 Developer Experience** — The first-ten-minutes story: one-line install, `pulsate init` with automatic app detection, per-framework recipes (Rails/Node/Go/Rust/static), Docker/Compose/Kubernetes integration, and dev/debug modes (local trusted HTTPS, browser diagnostics, live inspector).

**15 Observability** — Built-in metrics, OpenTelemetry tracing with per-stage spans and context propagation, Prometheus exposition, structured logs, and request IDs — all correlated so any request is followable end-to-end.

**16 Deployment** — Running everywhere with the same binary/config: bare metal, a hardened systemd unit, Docker/Compose, Kubernetes (Gateway API + native CRD), cloud, and coordinated multi-node clusters with shared certs/cache/limits, plus zero-downtime upgrades.

**17 Documentation** — A Diátaxis-structured docs program where reference docs are generated from the binary (config/CLI/API/metrics/errors) so they cannot drift, plus tested tutorials, examples, ADRs, and the contribution guide.

**18 Open Source** — Apache-2.0 with a published open-core boundary ("no rug-pulls"), DCO-based contribution, an evolving governance model, the Contributor Covenant, the RFC process, issue/PR templates, and a coordinated-disclosure security policy.

**19 Milestones** — Nine sequenced phases (P0 foundations → P8 cluster/K8s/1.0) each with objectives, LOC and duration estimates (~107k core Rust LOC, ~Year 1 to 0.x, ~Year 2 to 1.0), deliverables, testing requirements, risks, dependencies, and binary acceptance criteria.

**20 Future** — The additive commercial/ecosystem arc: enterprise edition, an optional cloud control plane (never in the request path), a managed service, support, the plugin marketplace, AI-assisted diagnostics, and multi-region active-active — structured so commercial success requires the open core to thrive.

**21 Threat Model** — STRIDE across seven trust boundaries, the attack surface and abuse cases, and a threat→mitigation matrix tying each risk to a control in doc 09.

**22 Admin API** — The versioned REST+gRPC control plane: auth/RBAC scopes, problem+json errors, and every endpoint for config, state, certs, cache, upstreams, events (SSE/WS), cluster, and diagnostics.

**23 Data & State Model** — The `ConfigSnapshot`, in-memory concurrent structures, the redb-backed persistent store, the on-disk layout, encryption at rest, and cluster-shared state with per-datum consistency.

**24 ADRs** — Twenty immutable decision records (Rust, Tokio+`pulsate-rt`, the snapshot split, Pulsate Flow, rustls, WASM plugins, redb, Apache-2.0, the `~>` operator, independent versioning, …) each with context, alternatives, and consequences.

**25 Error & Status Catalog** — The stable `PLS-<AREA>-<NNNN>` taxonomy, RFC-9457 problem+json shape, per-subsystem code tables, and process exit codes — generated from the same registry that drives behavior.

**26 Metrics & SLO Catalog** — Every metric (name/type/unit/labels) by subsystem with a hard cardinality budget, recommended SLIs/SLOs (notably *Pulsate-overhead* latency), Prometheus rules, and the Grafana layout.

**27 Configuration Reference** — The exhaustive, schema-generated key-by-key reference for every Flow block, directive, and field (types, defaults, constraints).

**28 Testing & Conformance** — The adversarial-weighted test strategy: property tests, continuous fuzzing, golden diagnostics, the integration harness, protocol conformance (h2spec/h3spec/RFC-9111), TLS/ACME interop (Pebble), load/soak/chaos, plugin sandbox tests, and release gates.

**29 Multi-Tenancy & Isolation** — The tenant model, host/cache/limit namespacing, per-tenant quotas and noisy-neighbor protection, RBAC-scoped control plane, isolated telemetry/audit, per-tenant plugin sandboxing, and blast-radius containment.

**30 Migration & Import** — `pulsate import` for nginx/Caddy/HAProxy/Apache with directive mapping tables, a fidelity model (exact/approximate/manual/dropped), a safe shadow-then-cutover workflow, and honest limitations.

**31 Benchmarking & Tuning** — Reference environments, the workload matrix, correct-latency tooling, fair apples-to-apples comparison methodology, no-cherry-pick reporting, and a practical OS/config/topology tuning guide.

**32 Disaster Recovery & HA** — Single-node resilience, multi-node HA, backup/restore (incl. ACME account-key recovery), per-datum consistency, RPO/RTO targets, upgrade/rollback, split-brain handling, and operational runbooks.

**33 Release Engineering & Supply Chain** — Reproducible hermetic builds, CycloneDX SBOMs, Sigstore signing + SLSA provenance, `cargo-deny`/`audit`/`vet` dependency policy, release channels, distribution, verified self-updates, and the plugin supply chain.

## How this plan is structured

The plan was authored as a set of self-consistent documents anchored to a single canonical set of decisions (names, the Flow config syntax, the request-lifecycle stage names, the crate list, and library choices) so terminology and interfaces line up across all 34 files. Core documents (01–20) map one-to-one to the required topic areas; the deep-dive references (21–33) add the reference material — threat model, API/data/error/metric catalogs, exhaustive config reference, testing, multi-tenancy, migration, benchmarking, DR/HA, and supply chain — that an engineering team needs to actually build, operate, and harden Pulsate. Cross-references at the foot of every document let you navigate by concept rather than by file number.

No implementation code is included by design: this is the plan a team builds *from*. The first build step is [03. Repository](03-repository.md) (scaffold the workspace) following the phase order in [19. Milestones](19-milestones.md).
