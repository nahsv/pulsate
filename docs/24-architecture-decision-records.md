# 24. Architecture Decision Records

> The formal record of *why* Pulsate is built the way it is. Each ADR captures a decision's context, the choice, the alternatives weighed, and the consequences. ADRs are immutable once accepted; a reversal is a new ADR that supersedes.

**Contents**
- [Format & status](#format--status)
- [Index](#index)
- [The records](#the-records)
- [Cross-references](#cross-references)

---

## Format & status

Each ADR: **Context** (forces at play) → **Decision** → **Alternatives** (and why rejected) → **Consequences** (good and bad). Status ∈ {Proposed, Accepted, Superseded(by N), Deprecated}. ADRs are the durable outputs of the [RFC process](18-open-source.md) and are published in the docs ([17. Documentation](17-documentation.md)).

## Index

| # | Title | Status |
|---|---|---|
| 0001 | Rust as the implementation language | Accepted |
| 0002 | Tokio runtime behind a `pulsate-rt` abstraction | Accepted |
| 0003 | In-process control/data-plane split via an immutable snapshot | Accepted |
| 0004 | `arc-swap` snapshot publication for lock-free hot reload | Accepted |
| 0005 | An original config language (Pulsate Flow) over YAML/templating | Accepted |
| 0006 | Hand-written parser over a parser-generator | Accepted |
| 0007 | rustls (no OpenSSL) | Accepted |
| 0008 | hyper for H1/H2, quinn+h3 for H3 | Accepted |
| 0009 | WebAssembly (Wasmtime, component model) for plugins | Accepted |
| 0010 | redb as the embedded state store | Accepted |
| 0011 | Cargo workspace with a layered crate graph | Accepted |
| 0012 | Apache-2.0 license + open-core | Accepted |
| 0013 | Embedded Svelte dashboard over a separate service | Accepted |
| 0014 | The `~>` flow operator as the route model | Accepted |
| 0015 | Independent versioning: binary / `flow_version` / plugin ABI | Accepted |
| 0016 | Tail-latency and bounded-memory as first-class acceptance criteria | Accepted |
| 0017 | Prometheus + OpenTelemetry as the only telemetry standards | Accepted |
| 0018 | Stable error-code taxonomy (`PLS-*`) + problem+json | Accepted |
| 0019 | Secure-by-default posture (opt-out, not opt-in) | Accepted |
| 0020 | Reserve a future external control plane (not required for a node) | Accepted |

## The records

**ADR-0001 — Rust.** *Context:* a proxy is security- and performance-critical; C/C++ proxies carry a long CVE tail from memory bugs; GC languages add latency jitter. *Decision:* implement in Rust. *Alternatives:* Go (GC pauses, weaker zero-cost abstractions — rejected for tail latency), C/C++ (memory unsafety — rejected), Zig (immature ecosystem). *Consequences:* memory safety + C-class performance; steeper contributor ramp; must manage `unsafe` carefully ([03. Repository](03-repository.md)).

**ADR-0002 — Tokio behind `pulsate-rt`.** *Context:* need a mature async ecosystem now, want a thread-per-core io_uring future. *Decision:* default to Tokio, isolate runtime primitives behind `pulsate-rt`. *Alternatives:* glommio/monoio only (less portable today — rejected as the default), custom executor (huge cost). *Consequences:* ecosystem leverage now, a migration path later without rewriting the data plane ([02. Architecture](02-architecture.md#async-runtime)).

**ADR-0003 — In-process snapshot split.** *Context:* want Envoy's control/data discipline without an external xDS control plane. *Decision:* split in-process; the only shared object is an immutable snapshot. *Alternatives:* external control plane (operational tax — rejected for the base case), no split (untestable, unsafe reloads). *Consequences:* safe reloads, testable boundary, single self-sufficient binary; an external control plane becomes an *optional* future layer ([20. Future](20-future.md)).

**ADR-0004 — `arc-swap` reload.** *Context:* reloads must not drop connections or stall readers. *Decision:* publish config via `ArcSwap`; in-flight requests keep their `Arc`. *Alternatives:* RwLock (reader stalls — rejected), process restart/fork-drain (heavier, drops or doubles resources). *Consequences:* lock-free reads, atomic swaps, one retained prior generation for rollback.

**ADR-0005 — Pulsate Flow.** *Context:* config is the primary UX; YAML/templating cause the pain Pulsate exists to remove. *Decision:* an original, typed, declarative DSL with the `~>` route model. *Alternatives:* YAML/JSON (no domain semantics, footguns), Caddyfile-like (not original; ordering subtleties), HCL (heavier, not request-shaped). *Consequences:* great error messages and ergonomics; cost of building/maintaining a language and tooling ([04. Configuration](04-configuration.md)).

**ADR-0006 — Hand-written parser.** *Context:* diagnostic quality is a feature. *Decision:* hand-written lexer + recursive descent with spans. *Alternatives:* pest/nom/lalrpop (worse error messages, less control). *Consequences:* precise `PLS-CFG-*` diagnostics; more code to own and fuzz.

**ADR-0007 — rustls.** *Context:* one-binary, memory-safe. *Decision:* rustls only. *Alternatives:* OpenSSL (C dep, CVE history, breaks one-binary), native-tls. *Consequences:* memory-safe TLS, modern defaults; must add a FIPS-validated provider for regulated builds ([09. Security](09-security.md)).

**ADR-0008 — hyper + quinn/h3.** *Context:* correctness over reinvention for HTTP. *Decision:* hyper (H1/H2), quinn+h3 (H3). *Alternatives:* hand-rolled HTTP (correctness/maintenance risk), msquic (C dep). *Consequences:* battle-tested correctness; depend on upstream cadence; a thin normalization layer unifies them ([05. HTTP Stack](05-http-stack.md)).

**ADR-0009 — WASM plugins.** *Context:* extensibility without forking or unsafe native loading. *Decision:* Wasmtime + component model + WIT, capability-sandboxed. *Alternatives:* native dlopen (no sandbox), Go-plugin/recompile (breaks one-binary), Lua (one language, ceiling). *Consequences:* safe, language-agnostic, hot-loadable extensions; ABI to maintain; marshaling overhead with a native escape hatch ([12. Plugins](12-plugins.md)).

**ADR-0010 — redb.** *Context:* need embedded, transactional, pure-Rust state. *Decision:* redb. *Alternatives:* SQLite (C dep), sled (stability), files-only (no transactions). *Consequences:* ACID local state, no C dep; smaller ecosystem than SQLite ([23. Data & State Model](23-data-and-state-model.md)).

**ADR-0011 — Workspace + layering.** *Context:* want embeddability and enforced architecture. *Decision:* one workspace, layered crates, data plane independent of control plane. *Alternatives:* single mega-crate (no layering, slow builds), many repos (coordination cost). *Consequences:* parallel builds, clear seams, enforceable layering ([03. Repository](03-repository.md)).

**ADR-0012 — Apache-2.0 + open-core.** *Context:* infra adoption + a durable business. *Decision:* Apache-2.0 core, separate commercial edition. *Alternatives:* MIT (no patent grant), AGPL/SSPL core (chills adoption). *Consequences:* broad adoption + patent protection; must hold the open/closed line publicly ([18. Open Source](18-open-source.md)).

**ADR-0013 — Embedded Svelte dashboard.** *Context:* one binary, no extra process. *Decision:* compile a Svelte SPA into the binary, serve via the control plane. *Alternatives:* separate Node service (breaks one-binary), heavy SPA framework (bundle size). *Consequences:* zero-dependency UI; frontend build in the pipeline ([11. Dashboard](11-dashboard.md)).

**ADR-0014 — The `~>` flow operator.** *Context:* a route is "match → pipeline → handler"; make config mirror the request. *Decision:* left-to-right `~>` chains. *Alternatives:* nested blocks (verbose), ordered directive lists (hidden ordering). *Consequences:* readable, order-explicit routes; a novel syntax to teach ([04. Configuration](04-configuration.md), [07. Middleware](07-middleware.md)).

**ADR-0015 — Independent versioning.** *Context:* upgrading the binary shouldn't break configs or plugins. *Decision:* version binary (SemVer), `flow_version`, and plugin ABI independently. *Alternatives:* one version for all (forces lockstep upgrades). *Consequences:* smooth upgrades; more compatibility matrices to test ([03. Repository](03-repository.md#versioning)).

**ADR-0016 — Tail latency & bounded memory as acceptance criteria.** *Context:* a proxy's value is consistency and safety under load. *Decision:* p99/p999 and bounded-memory targets gate releases. *Alternatives:* optimize mean throughput only (misleading). *Consequences:* CI perf gates, soak tests; some features blocked until they meet the bar ([10. Performance](10-performance.md)).

**ADR-0017 — Prometheus + OTel only.** *Context:* avoid lock-in, fit existing stacks. *Decision:* support exactly the open standards. *Alternatives:* proprietary agent (lock-in), many bespoke exporters (maintenance). *Consequences:* universal compatibility; rely on the OTel collector for exotic backends ([15. Observability](15-observability.md)).

**ADR-0018 — Error taxonomy.** *Context:* operators need stable, searchable errors. *Decision:* `PLS-<AREA>-<NNNN>` codes + RFC 9457 problem+json. *Alternatives:* free-text errors (unsearchable, unstable). *Consequences:* scriptable, documentable errors; discipline to assign/maintain codes ([25. Error Catalog](25-error-and-status-catalog.md)).

**ADR-0019 — Secure by default.** *Context:* misconfiguration is the top real-world risk. *Decision:* safe defaults, explicit opt-out. *Alternatives:* flexible-but-unsafe defaults. *Consequences:* safe out-of-box; occasional friction when users genuinely need the unsafe path (made explicit) ([09. Security](09-security.md)).

**ADR-0020 — Reserve an external control plane.** *Context:* fleets eventually want central management; nodes must stay self-sufficient. *Decision:* keep any external control plane optional and out of the request path. *Alternatives:* require a control plane (Envoy's tax). *Consequences:* a node always works alone; the cloud control plane is additive ([20. Future](20-future.md)).

## Cross-references
- [02. Architecture](02-architecture.md) — decisions 0002–0004, 0020 in practice.
- [04. Configuration](04-configuration.md) — decisions 0005, 0006, 0014.
- [18. Open Source](18-open-source.md) — the RFC process that produces ADRs; 0012.
- [12. Plugins](12-plugins.md), [09. Security](09-security.md), [10. Performance](10-performance.md) — 0009, 0019, 0016.
