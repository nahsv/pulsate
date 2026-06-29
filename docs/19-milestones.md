# 19. Milestones

> The execution plan: Pulsate broken into sequenced phases, each with objectives, an estimated lines-of-code budget, duration, deliverables, testing requirements, risks, dependencies, and acceptance criteria — enough for a team to plan sprints and track progress.

**Contents**
- [How to read this](#how-to-read-this)
- [Phase timeline & dependencies](#phase-timeline--dependencies)
- [Phase 0 — Foundations & scaffolding](#phase-0--foundations--scaffolding)
- [Phase 1 — Config language & snapshot core](#phase-1--config-language--snapshot-core)
- [Phase 2 — HTTP/1.1 + TLS data plane](#phase-2--http11--tls-data-plane)
- [Phase 3 — Reverse proxy & middleware](#phase-3--reverse-proxy--middleware)
- [Phase 4 — ACME, HTTP/2 & observability](#phase-4--acme-http2--observability)
- [Phase 5 — Cache & security (WAF)](#phase-5--cache--security-waf)
- [Phase 6 — Admin API & dashboard](#phase-6--admin-api--dashboard)
- [Phase 7 — HTTP/3 & plugins](#phase-7--http3--plugins)
- [Phase 8 — Clustering, K8s & 1.0 hardening](#phase-8--clustering-k8s--10-hardening)
- [Estimates summary](#estimates-summary)
- [Cross-cutting risks](#cross-cutting-risks)
- [Cross-references](#cross-references)

---

## How to read this

- **LOC estimates** are order-of-magnitude Rust LOC (excluding tests/docs, which add ~60–100%); they size effort, not a target to hit.
- **Durations** assume a small senior team (~3–5 engineers) and are calendar estimates; phases overlap where dependencies allow (see timeline).
- **Acceptance criteria** are binary and testable — a phase is "done" only when all are met and its testing requirements pass in CI.
- Phases map onto the crate set in [03. Repository](03-repository.md) and the roadmap arc in [01. Vision](01-vision.md).

## Phase timeline & dependencies

```
P0 Foundations ─▶ P1 Config core ─▶ P2 HTTP/1.1+TLS ─▶ P3 Proxy+Middleware ─▶ P4 ACME+H2+Obs
                                          │                     │                    │
                                          └─────────────────────┴───────▶ P5 Cache+WAF
                                                                          │
                                                          P6 Admin+Dashboard ─▶ P7 H3+Plugins ─▶ P8 Cluster+K8s+1.0
   ◀────────── ~Year 1 (P0–P4, 0.x public) ──────────▶◀──────── ~Year 2 (P5–P8, → 1.0) ────────▶
```

Critical path runs P0→P1→P2→P3; P4/P5 parallelize once the proxy path exists; P6 can start after the Admin API contract (P4) is drafted.

---

## Phase 0 — Foundations & scaffolding

- **Objectives:** establish the workspace, CI, coding standards, the runtime abstraction, and the core vocabulary so all later work plugs in cleanly.
- **Deliverables:** Cargo workspace + all crate stubs; `pulsate-core` types/traits/error taxonomy skeleton; `pulsate-rt` Tokio adapter; `pulsate-util` buffer pool; CI pipeline (fmt/clippy/deny/test/doc/MSRV matrix); `xtask`; contribution/governance docs scaffolding.
- **LOC:** ~3k. **Duration:** ~3–4 weeks.
- **Testing:** CI green on all platforms; trait/doc-test stubs compile; `cargo deny` passes.
- **Risks:** over-designing core traits before real usage (mitigate: keep `#[non_exhaustive]`, iterate).
- **Dependencies:** none.
- **Acceptance:** `cargo build`/`test`/`clippy`/`doc` pass on the full matrix; crate graph enforces layering; a "hello" bin runs.

## Phase 1 — Config language & snapshot core

- **Objectives:** implement Pulsate Flow end-to-end and the snapshot/reload machinery — the backbone everything configures against.
- **Deliverables:** `pulsate-flow` lexer/parser/AST with span-accurate diagnostics; `pulsate-config` typed model, includes/env/secret resolution, validation, `ConfigSnapshot` build + `arc-swap` publish + reload/rollback; `pulsate validate`/`fmt`/`config dump|diff|explain`.
- **LOC:** ~8k. **Duration:** ~6–8 weeks.
- **Testing:** parser unit + **property** (round-trip) + **fuzz** tests; golden-file validation tests; reload-under-concurrency tests; diagnostic snapshot tests.
- **Risks:** config UX/error-quality is make-or-break (mitigate: invest early in diagnostics, dogfood); over-scoping the language (mitigate: stabilize `flow_version "1"` minimal surface first).
- **Dependencies:** P0.
- **Acceptance:** every [04. Configuration](04-configuration.md) example parses/validates; invalid configs produce `PLS-CFG-*` errors with correct spans; reload swaps snapshots atomically with zero reader stalls in tests.

## Phase 2 — HTTP/1.1 + TLS data plane

- **Objectives:** terminate HTTP/1.1 over TLS and serve static files — the first end-to-end request path through the [lifecycle](02-architecture.md#request-lifecycle).
- **Deliverables:** `pulsate-net` listeners (+`SO_REUSEPORT`, limits); `pulsate-tls` rustls termination, SNI, ALPN, manual certs, mTLS; `pulsate-http` H1 via hyper; `pulsate-router` matchers + precedence; a `files()` handler; `pulsate up`/`run`/`down` lifecycle + graceful shutdown.
- **LOC:** ~10k. **Duration:** ~8–10 weeks.
- **Testing:** integration tests via `pulsate-test` loopback harness; TLS interop matrix; **fuzz** the H1 decoder; request-smuggling/ambiguous-framing rejection tests; graceful-shutdown drain tests; routing precedence tests.
- **Risks:** HTTP correctness edge cases (mitigate: conformance suite, fuzzing); timeout/backpressure bugs (mitigate: explicit timeout tests).
- **Dependencies:** P1.
- **Acceptance:** serves real static sites over HTTPS with manual certs; passes the H1 conformance subset; no dropped connections on shutdown/reload; memory flat under a 1-hour soak.

## Phase 3 — Reverse proxy & middleware

- **Objectives:** the proxy core and the middleware pipeline — Pulsate's reason to exist.
- **Deliverables:** `pulsate-pipeline` Ingress/Egress/Recover driver + built-ins (headers, rewrite, cors, compress, basic_auth, redirect, respond); `pulsate-proxy` upstream pools, LB policies, retries, circuit breakers, active/passive health checks, static + DNS discovery; `proxy()`/`ws()` handlers; forwarded-header handling.
- **LOC:** ~12k. **Duration:** ~10–12 weeks.
- **Testing:** proxy integration tests with fake/faulty upstreams; LB distribution tests; retry-budget/breaker state-machine tests; health-check flap/hysteresis tests; WebSocket passthrough; chaos (kill upstreams under load).
- **Risks:** retry storms / breaker misconfiguration causing outages (mitigate: budgets, conservative defaults, chaos tests); pool/keep-alive correctness (mitigate: soak + leak detection).
- **Dependencies:** P2.
- **Acceptance:** proxies a multi-backend service with health-aware LB; survives backend failure (ejection + breaker + retry) without cascading; proxy overhead within target ([10. Performance](10-performance.md)); pipeline ordering matches [07. Middleware](07-middleware.md).

## Phase 4 — ACME, HTTP/2 & observability

- **Objectives:** make TLS automatic, add HTTP/2, and light up metrics/traces/logs — the "secure & observable by default" promise.
- **Deliverables:** `pulsate-acme` HTTP-01/TLS-ALPN-01/DNS-01, renewal, cert store, on-demand issuance; HTTP/2 in `pulsate-http` (+abuse defenses); `pulsate-observe` metrics + Prometheus exporter, `tracing`+OTLP, structured/access logs, request IDs; `pulsate-secrets` (env/file/Vault).
- **LOC:** ~10k. **Duration:** ~8–10 weeks (parallel with late P3).
- **Testing:** ACME against **Pebble** (staging) in CI; renewal/rollover tests; H2 conformance (**h2spec**) + rapid-reset/flood DoS tests; metrics/trace correlation tests; secret redaction tests.
- **Risks:** ACME edge cases & rate limits (mitigate: staging, backoff, on-demand allow-listing); H2 DoS classes (mitigate: explicit mitigations + tests — [09. Security](09-security.md)).
- **Dependencies:** P2 (TLS), P3 (for meaningful metrics).
- **Acceptance:** `tls auto` obtains and renews a real cert (staging in CI, prod in a manual gate); h2spec passes; a request is traceable end-to-end across logs/metrics/trace by request ID.

> **Milestone: public 0.x release** after P4 — a genuinely useful single-node gateway. Begins external feedback and dogfooding.

## Phase 5 — Cache & security (WAF)

- **Objectives:** batteries — full HTTP caching and the WAF/rate-limit/geo/bot suite.
- **Deliverables:** `pulsate-cache` memory/disk/redis stores, RFC-9111 freshness, validators, SWR/stale-if-error, ranges, tags/purge, compression-aware caching, single-flight; `pulsate-waf` rule engine (CRS-compatible), rate limiting (local+distributed), geo/ASN/bot/IP controls; audit logging.
- **LOC:** ~12k. **Duration:** ~10–12 weeks.
- **Testing:** RFC-9111 conformance vectors; cache correctness (vary/validators/ranges) tests; purge-propagation tests; WAF true/false-positive corpus; rate-limit accuracy under concurrency; audit-log integrity (hash-chain) tests.
- **Risks:** cache correctness bugs serving wrong content (mitigate: spec vectors, fuzzing keys/vary); WAF false positives (mitigate: detect-mode rollout, tuning, scoring).
- **Dependencies:** P3 (pipeline/proxy), P4 (metrics).
- **Acceptance:** cache passes RFC-9111 vectors and demonstrably offloads origin; WAF blocks the OWASP test corpus with bounded false positives; distributed rate limit is accurate within tolerance across nodes; audit log is tamper-evident.

## Phase 6 — Admin API & dashboard

- **Objectives:** the operable control surface — Admin API + embedded dashboard.
- **Deliverables:** `pulsate-control` REST+gRPC Admin API (config apply/diff, runtime state, cert/cache/upstream ops, events SSE, request-inspector WS) with authn/RBAC; `pulsate-dashboard` Svelte SPA (overview, metrics, live logs, config editor, cert manager, request inspector, cache stats), embedded; CLI commands wired to the API.
- **LOC:** ~12k (Rust) + ~10k (frontend, TS/Svelte). **Duration:** ~10–12 weeks (parallelizable; frontend track concurrent).
- **Testing:** Admin API contract tests (OpenAPI conformance); RBAC/authz tests; dashboard E2E (Playwright) against a live binary; config-editor apply/rollback safety tests.
- **Risks:** admin surface as attack surface (mitigate: loopback default, authz tests, audit, [21. Threat Model](21-threat-model.md)); frontend scope creep (mitigate: API-first, MVP views).
- **Dependencies:** P4 (Admin API contract, metrics), P5 (cache/WAF data to show).
- **Acceptance:** the dashboard performs every privileged action via the API with RBAC enforced and audited; config editing is validate→diff→apply→rollback safe; the request inspector shows full lifecycle attribution.

## Phase 7 — HTTP/3 & plugins

- **Objectives:** modern transport (H3/QUIC) and the extensibility story (WASM plugins + SDK).
- **Deliverables:** `pulsate-http3` QUIC/h3 (quinn), Alt-Svc, 0-RTT policy, UDP perf (GSO/GRO); `pulsate-plugin` Wasmtime host (component model, WIT host API, capability sandbox, fuel/epoch, pooling, signing/verify); `pulsate-sdk` Rust SDK + templates; `pulsate plugin` CLI.
- **LOC:** ~14k. **Duration:** ~12–14 weeks.
- **Testing:** **h3spec**, QUIC interop, 0-RTT replay tests; plugin sandbox-escape attempts, fuel/epoch limit tests, ABI-version compat tests, signing/verify tests; plugin perf benchmarks.
- **Risks:** QUIC/UDP perf and portability (mitigate: feature-gate, fallbacks); plugin ABI stability commitment (mitigate: version it conservatively, deprecation policy — [12. Plugins](12-plugins.md)); sandbox correctness (mitigate: adversarial tests, capability-deny-by-default).
- **Dependencies:** P2 (TLS/listeners), P3 (pipeline for plugin-as-middleware).
- **Acceptance:** H3 passes h3spec and serves real traffic with Alt-Svc upgrade; a third-party-style WASM plugin loads, runs sandboxed within limits, and cannot exceed its granted capabilities; ABI is documented and versioned.

## Phase 8 — Clustering, K8s & 1.0 hardening

- **Objectives:** multi-node operation, Kubernetes-native integration, and the hardening to call it 1.0.
- **Deliverables:** `pulsate-cluster` membership/gossip, shared cert issuance (leader election), distributed cache/rate-limit/sticky; Kubernetes Gateway API + native CRD controller + EndpointSlice discovery + Helm/operator; `pulsate-migrate` (nginx/Caddy/Traefik); zero-downtime binary upgrade (socket handoff); full docs, conformance, soak, and security audit.
- **LOC:** ~16k. **Duration:** ~12–16 weeks.
- **Testing:** multi-node integration (cert sharing, purge propagation, split-brain); K8s e2e (kind) incl. Gateway API conformance; migration fidelity tests vs real configs; long-duration soak + chaos; **external security audit** + fuzzing sweep; full benchmark vs competitors published.
- **Risks:** distributed-systems correctness (mitigate: deterministic sim tests, jepsen-style checks where applicable); 1.0 API/format stability commitment (mitigate: freeze + deprecation policy); audit findings late (mitigate: continuous security review from P4).
- **Dependencies:** P5 (shared cache/limits), P6 (admin/config distribution), P7 (complete feature set).
- **Acceptance:** a 3-node cluster shares certs/cache/limits and survives node loss with no correctness loss; Pulsate passes Gateway API conformance; migrations import representative real-world configs with documented fidelity; security audit issues resolved; performance targets met and published → **tag 1.0** with stable config format and plugin ABI.

## Estimates summary

| Phase | Focus | Core LOC | Duration |
|---|---|---|---|
| P0 | Foundations | ~3k | 3–4 wk |
| P1 | Config core | ~8k | 6–8 wk |
| P2 | H1 + TLS | ~10k | 8–10 wk |
| P3 | Proxy + middleware | ~12k | 10–12 wk |
| P4 | ACME + H2 + observ. | ~10k | 8–10 wk |
| P5 | Cache + WAF | ~12k | 10–12 wk |
| P6 | Admin + dashboard | ~22k | 10–12 wk |
| P7 | H3 + plugins | ~14k | 12–14 wk |
| P8 | Cluster + K8s + 1.0 | ~16k | 12–16 wk |
| **Total** | core ~**107k** Rust LOC (+~10k frontend, +tests/docs) | | ~**Year 1**: P0–P4 (0.x), **Year 2**: P5–P8 (1.0) |

These align with the [01. Vision](01-vision.md) roadmap (Y1 foundations, Y2 platform/extensibility).

## Cross-cutting risks

- **Scope discipline:** "batteries included" invites endless scope. Mitigate with the RFC process ([18. Open Source](18-open-source.md)) and a hard line on the 1.0 surface.
- **Correctness under concurrency:** the snapshot/lock-free model is powerful but subtle. Mitigate with property/fuzz/soak/chaos from the start, not the end ([28. Testing](28-testing-and-conformance.md)).
- **Security debt:** continuous security review and the threat model ([21](21-threat-model.md)) from P4 onward, not a single pre-1.0 audit.
- **Performance regressions:** the CI perf-gate ([10. Performance](10-performance.md)) runs from P2 so regressions are caught per-PR.
- **Stability promises:** config-format and plugin-ABI stability are commitments; version them independently and freeze deliberately ([03. Repository](03-repository.md)).

## Cross-references
- [01. Vision](01-vision.md) — the multi-year roadmap these phases execute.
- [03. Repository](03-repository.md) — crates each phase builds; CI/release gates.
- [28. Testing & Conformance](28-testing-and-conformance.md) — the test classes referenced as requirements.
- [10. Performance](10-performance.md) — performance acceptance criteria.
- [20. Future](20-future.md) — what comes after 1.0.
