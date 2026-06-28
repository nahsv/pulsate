# 28. Testing and Conformance

> The deep test strategy: protocol conformance, fuzzing, property and golden tests, load/soak/chaos, ACME/TLS interop, plugin sandbox testing, coverage targets, and the CI test pyramid — mapped to crates and used as release gates.

**Contents**
- [Testing philosophy](#testing-philosophy)
- [The test pyramid](#the-test-pyramid)
- [Unit & property tests](#unit--property-tests)
- [Fuzzing](#fuzzing)
- [Golden & snapshot tests](#golden--snapshot-tests)
- [Integration tests & the harness](#integration-tests--the-harness)
- [Protocol conformance](#protocol-conformance)
- [TLS/ACME interop](#tlsacme-interop)
- [Load, soak & chaos](#load-soak--chaos)
- [Plugin & sandbox testing](#plugin--sandbox-testing)
- [Coverage, gates & release criteria](#coverage-gates--release-criteria)
- [Cross-references](#cross-references)

---

## Testing philosophy

A proxy fails in production in ways unit tests miss: adversarial inputs, protocol corner cases, behavior under sustained load, and partial failures. Pulsate therefore weights its testing toward **adversarial and systemic** classes (fuzz, conformance, chaos, soak), not just example-based unit tests. Three rules: every bug becomes a regression test; correctness/perf gates block releases (not just CI green); and tests are mapped to the crates they cover so gaps are visible.

## The test pyramid

```
                 ┌───────────────┐  few, slow, high-fidelity
                 │  E2E / chaos  │  (p8 up scenarios, kill upstreams, K8s)
              ┌──┴───────────────┴──┐
              │ conformance / soak  │ (h2spec/h3spec/RFC-9111, multi-hour)
           ┌──┴─────────────────────┴──┐
           │   integration (harness)   │ (real listeners + fake upstreams)
        ┌──┴───────────────────────────┴──┐
        │  fuzz + property + golden        │ (parsers, decoders, matchers)
     ┌──┴─────────────────────────────────┴──┐
     │            unit tests                  │  many, fast
     └────────────────────────────────────────┘
```

## Unit & property tests

- **Unit** (`#[cfg(test)]` per crate): parser tokens, router precedence, cache freshness math, LB selection, retry/breaker state machines, header rewriting, duration/size parsing.
- **Property** (`proptest`): invariants over random inputs —
  - Flow: `parse(print(ast)) == ast` (round-trip); any valid AST validates or yields a precise error.
  - Router: matching is deterministic and order-independent; the most-specific rule always wins.
  - Cache: key composition is stable; freshness never serves a `no-store` response.
  - LB: weighted distribution converges to configured weights.
- **Microbenchmarks** (`criterion`) guard inner-loop performance ([10. Performance](10-performance.md)).

## Fuzzing

Continuous fuzzing (`cargo-fuzz`/libFuzzer, plus structure-aware `arbitrary`) on the highest-risk surfaces:
- **Flow parser** — must never panic; always produce an AST or a spanned error.
- **HTTP/1.1 decoder** — never panic, never accept ambiguous framing (smuggling corpus).
- **HTTP/2 & HPACK / HTTP/3 & QPACK** frame decoders.
- **TLS record path** (via rustls fuzz harnesses) and **WAF rule matcher** (ReDoS-resistant).
- **Config snapshot builder** — random valid configs never produce inconsistent snapshots.
Corpora are seeded from real traffic captures and grown in CI; crashes auto-file regression tests. Fuzz smoke runs per-PR (short), deep runs nightly/pre-release ([19. Milestones](19-milestones.md)).

## Golden & snapshot tests

- **Config diagnostics:** a corpus of invalid `.flow` files with expected `PLS-CFG-*` output (code + span + hint) — locks down error-message quality ([04. Configuration](04-configuration.md)).
- **Effective config:** golden `p8 config dump --effective` outputs for representative inputs.
- **Routing tables:** golden compiled-route dumps assert precedence didn't regress.
- **Generated docs/specs:** OpenAPI, config schema, metrics/error catalogs are snapshot-tested so docs match the binary ([17. Documentation](17-documentation.md)).

## Integration tests & the harness

`pulsate-test` provides a reusable harness:
- **Fake upstreams:** programmable HTTP/gRPC/WebSocket backends that can be slow, flaky, return specific statuses, drop connections, or send malformed responses.
- **Loopback server:** spin up a real Pulsate on an ephemeral port with a given `.flow`, drive real client requests (h1/h2/h3, TLS), assert responses, headers, timing, and metrics.
- **Scenarios:** end-to-end routing, middleware ordering, proxy + retries/breakers/health, cache hit/miss/SWR/SIE, WAF blocks, auth flows, reload-under-load (assert zero dropped connections), graceful shutdown drain.
Integration tests run per-PR on the platform matrix.

## Protocol conformance

Third-party conformance suites, run nightly and pre-release, gating releases:
- **HTTP/2:** `h2spec` (full) — framing, flow control, error handling.
- **HTTP/3:** `h3spec` + QUIC interop (against quinn/other implementations).
- **HTTP semantics (RFC 9110/9111):** caching vectors and method/semantic tests; a curated RFC-9111 cache-correctness vector set.
- **HTTP/1.1:** smuggling/framing test corpus (e.g., known smuggling payloads must be rejected).
- **WebSocket (RFC 6455) / SSE / gRPC:** autobahn-style WS suite, gRPC interop, gRPC-Web translation tests.

## TLS/ACME interop

- **TLS interop matrix:** handshake against a range of clients (browsers, curl/openssl versions, mobile stacks) across TLS 1.2/1.3, cipher presets, mTLS modes; assert correct ALPN and resumption.
- **ACME:** full issuance/renewal/rollover against **Pebble** (a local ACME test CA) in CI for HTTP-01/TLS-ALPN-01/DNS-01 (mock DNS), including failure/backoff and on-demand allow-listing; staging-CA tests gated behind manual approval; rate-limit-respect tests.
- **OCSP stapling** and cert-rotation-without-downtime tests.

## Load, soak & chaos

- **Load:** the benchmark matrix ([31. Benchmarking & Tuning](31-benchmarking-and-tuning.md)) run on fixed hardware; a CI subset gates on >X% regression.
- **Soak:** multi-hour runs sustaining traffic + periodic reloads + cache churn; assert **flat RSS** (no leaks), no fd growth, stable latency.
- **Chaos:** kill/restart upstreams under load (assert ejection+breaker+retry behave, no cascading failure), drop/delay packets (tc/netem), fill the cache disk, exhaust connections, reload repeatedly under load, kill a worker (assert supervisor recovery), partition a cluster (assert no split-brain corruption).
- **Distributed correctness:** deterministic-simulation / model-based tests for the cluster (membership, leader election, purge propagation) where feasible.

## Plugin & sandbox testing

- **Sandbox escape attempts:** adversarial plugins try to read host memory, exceed capabilities, hang (fuel/epoch), allocate unbounded, or call ungranted imports — all must fail safely with the right `PLS-PLG-*` code ([25. Error Catalog](25-error-and-status-catalog.md)).
- **ABI compatibility:** plugins built against each supported world version load on the current binary (version matrix).
- **Determinism & resource limits:** verify fuel/epoch caps, instance-pool reset (no state leakage between requests), and fail-open/closed policy.
- **Signing/provenance:** unsigned/tampered plugins are rejected when `require_signed` is set.

## Coverage, gates & release criteria

- **Coverage targets:** ≥85% line coverage on `pulsate-core` and data-plane crates; 100% of `PLS-*` error codes exercised by a test; every public config key has a validation test.
- **Per-PR gates:** fmt, clippy, deny, unit+property+integration, doc build, MSRV, minimal-versions, fuzz smoke, perf-diff.
- **Pre-release gates:** full conformance (h2spec/h3spec/RFC-9111), TLS/ACME interop, soak (flat RSS), chaos scenarios, full benchmark vs competitors, deep fuzz sweep, and (pre-1.0) an external security audit.
- **Release criteria:** all gates pass; zero known correctness regressions; performance targets met; security audit findings resolved ([19. Milestones](19-milestones.md)).

## Cross-references
- [03. Repository](03-repository.md) — CI/CD pipeline and the testing table.
- [19. Milestones](19-milestones.md) — per-phase testing requirements & acceptance.
- [10. Performance](10-performance.md) & [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md) — load/soak methodology.
- [21. Threat Model](21-threat-model.md) — adversarial cases the fuzz/chaos suites target.
- [12. Plugins](12-plugins.md) — the sandbox guarantees these tests verify.
