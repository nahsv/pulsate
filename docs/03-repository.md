# 03. Repository & Engineering

> The Cargo workspace: every crate and why it exists, the dependency graph, and the engineering standards (coding style, naming, testing, CI/CD, releases, versioning) that keep a multi-crate codebase coherent.

**Contents**
- [Workspace layout](#workspace-layout)
- [Crate catalog](#crate-catalog)
- [Dependency graph](#dependency-graph)
- [Coding standards](#coding-standards)
- [Naming conventions](#naming-conventions)
- [Testing strategy](#testing-strategy)
- [CI/CD architecture](#cicd-architecture)
- [Release strategy](#release-strategy)
- [Versioning](#versioning)
- [Cross-references](#cross-references)

---

## Workspace layout

Pulsate is a single Cargo **workspace** with ~24 member crates plus tooling. A workspace (not one giant crate) is chosen so that: compilation parallelizes and caches per crate; the data plane can be embedded as a dependency without the control plane; the public API surface (`pulsate-core`, `pulsate-sdk`) is physically separated from internals; and crate boundaries enforce the architecture's layering (a `pulsate-http` that cannot reach into `pulsate-control` because it does not depend on it).

```
p8/
├── Cargo.toml                # [workspace] members, shared deps via workspace.dependencies
├── rust-toolchain.toml       # pinned toolchain channel
├── deny.toml                 # cargo-deny: licenses, advisories, bans
├── crates/
│   ├── p8/                # the binary (bin target `p8`)
│   ├── pulsate-core/           # shared types, traits, error taxonomy, RequestCtx
│   ├── pulsate-rt/             # async runtime abstraction (Tokio backend)
│   ├── pulsate-util/           # buffers, time, small shared helpers
│   ├── pulsate-flow/           # Flow config language: lexer, parser, AST, validate
│   ├── pulsate-config/         # config model, snapshot build, hot-reload, sources
│   ├── pulsate-net/            # listeners, sockets, accept, L4 forward
│   ├── pulsate-tls/            # rustls integration, SNI, ALPN, mTLS, resumption
│   ├── pulsate-http/           # HTTP/1.1 + HTTP/2 (hyper), request/response model
│   ├── pulsate-http3/          # HTTP/3 + QUIC (quinn, h3)
│   ├── pulsate-router/         # routing table + matchers (host/path/regex/weighted)
│   ├── pulsate-pipeline/       # middleware engine + built-in middleware
│   ├── pulsate-proxy/          # reverse proxy: pools, LB, retry, breaker, health, discovery
│   ├── pulsate-cache/          # caching subsystem (memory/disk/redis, HTTP semantics)
│   ├── pulsate-waf/            # WAF, rate limiting, bot/geo/ASN
│   ├── pulsate-control/        # control-plane orchestrator + admin API server
│   ├── pulsate-acme/           # ACME client + certificate manager + cert store
│   ├── pulsate-cluster/        # clustering, membership, shared state
│   ├── pulsate-secrets/        # secrets backends (env/file/vault/cloud)
│   ├── pulsate-observe/        # metrics, tracing, logging, request IDs, OTel
│   ├── pulsate-plugin/         # WASM host (Wasmtime, component model, WIT host API)
│   ├── pulsate-sdk/            # guest-side SDK for plugin authors
│   ├── pulsate-dashboard/      # embedded SPA assets + dashboard backend glue
│   ├── pulsate-cli/            # command implementations (lib used by the bin)
│   ├── pulsate-migrate/        # importers: nginx / Caddy / HAProxy / Apache → Flow
│   ├── pulsate-k8s/            # Kubernetes Gateway API controller → live config
│   └── pulsate-test/           # test harness, fakes, conformance utilities (dev-dep)
├── xtask/                    # cargo-xtask: dev automation (gen, dist, bench)
├── examples/                 # runnable example configs and apps
├── docs/                     # this plan + user/dev documentation
└── .github/ or .ci/          # pipeline definitions
```

Shared dependencies are declared once in `[workspace.dependencies]` and inherited by members (`tokio.workspace = true`) so versions never drift across crates.

## Crate catalog

| Crate | Layer | Purpose | Key deps |
|---|---|---|---|
| `p8` | bin | Process entry; parses CLI, wires control+data plane, owns the supervisor | clap, all below |
| `pulsate-core` | shared | The vocabulary of the system: `RequestCtx`, `Request`/`Response`, `PulsateError` taxonomy, traits (`Middleware`, `Handler`, `Matcher`, `Upstream`, `CacheStore`, `SecretsBackend`, `CertStore`, `MetricsSink`), `ConfigSnapshot` type | bytes, http |
| `pulsate-rt` | shared | Runtime abstraction (`spawn`, timers, TCP/UDP, `spawn_blocking`); Tokio adapter; seam for future io_uring backend | tokio |
| `pulsate-util` | shared | Buffer pools, duration/size parsing, small lock-free helpers | bytes |
| `pulsate-flow` | config | The Flow language: hand-written lexer, recursive-descent parser, AST, span tracking, diagnostic rendering, schema validation | — (intentionally dependency-light) |
| `pulsate-config` | config | Typed `Config` model; resolve includes/env/secrets; build & diff `ConfigSnapshot`; reload orchestration; config sources | pulsate-flow, pulsate-core |
| `pulsate-net` | data | Listeners, socket options, `SO_REUSEPORT`, connection limits, L4 TCP/UDP forwarding | pulsate-rt |
| `pulsate-tls` | data | rustls server/client config, SNI cert selection, ALPN, session resumption, mTLS verification | rustls, pulsate-acme |
| `pulsate-http` | data | HTTP/1.1 + HTTP/2 via hyper; normalize to `Request`/`Response`; keep-alive, timeouts | hyper, h2 |
| `pulsate-http3` | data | HTTP/3 over QUIC | quinn, h3 |
| `pulsate-router` | data | Compile routes into a matcher trie/table; host/path/regex/method/header/weighted matching | regex, pulsate-core |
| `pulsate-pipeline` | data | Execute Ingress/Egress/Recover; built-in middleware (compress, cors, headers, auth, ratelimit glue) | pulsate-core, pulsate-waf, pulsate-cache |
| `pulsate-proxy` | data | Upstream pools, LB policies, retries, circuit breakers, active/passive health checks, service discovery, dynamic upstreams | pulsate-http, pulsate-rt |
| `pulsate-cache` | data | Memory/disk/Redis stores; HTTP cache semantics (freshness, validators, SWR, range, vary) | redb, redis (opt) |
| `pulsate-waf` | data | Rule engine, rate limiting, bot/geo/ASN, IP reputation | maxminddb, governor-like limiter |
| `pulsate-control` | control | Orchestrator: owns sources of truth, drives reloads, serves the admin API (REST+gRPC), readiness | axum/tonic, pulsate-config |
| `pulsate-acme` | control | ACME (instant-acme), challenge solvers (HTTP-01/TLS-ALPN-01/DNS-01), cert store & renewal | instant-acme, rustls |
| `pulsate-cluster` | control | Node membership (gossip), leader/peer roles, shared state replication | — |
| `pulsate-secrets` | control | `SecretsBackend` impls: env, file, Vault, cloud KMS; secret refs & rotation | (per backend) |
| `pulsate-observe` | shared | `metrics` facade + Prometheus exporter; `tracing` + OTLP; structured JSON logs; request IDs | tracing, opentelemetry, prometheus |
| `pulsate-plugin` | plugins | Wasmtime host; component model; WIT host interface; sandbox, fuel/epoch limits, capability grants | wasmtime |
| `pulsate-sdk` | plugins | Guest-side ergonomic API for writing plugins in Rust (and a basis for other-language bindings) | — (no_std-friendly) |
| `pulsate-dashboard` | dashboard | Embeds the built Svelte SPA (`rust-embed`); serves it; bridges to admin API + SSE/WS | rust-embed |
| `pulsate-cli` | tooling | Implementation of every subcommand (so the bin stays thin and the CLI is testable) | clap, pulsate-config |
| `pulsate-migrate` | tooling | Parse nginx/Caddy/HAProxy/Apache configs; map to Flow; emit fidelity notes | — (hand-written parsers) |
| `pulsate-k8s` | tooling | Kubernetes Gateway API controller; reconcile Gateway/HTTPRoute into a live `ConfigSnapshot` via the admin reload path | kube, k8s-openapi |
| `pulsate-test` | shared (dev) | Test fixtures, fake upstreams, a loopback harness, conformance helpers | — |

## Dependency graph

Arrows mean "depends on." The graph is a DAG with `pulsate-core` at the root and the binary at the top. The data plane never depends on the control plane.

```
                                   p8 (bin)
                                      │
        ┌──────────────┬─────────────┼───────────────┬───────────────┐
        ▼              ▼             ▼               ▼               ▼
   pulsate-cli      pulsate-control  pulsate-dashboard  pulsate-http3     pulsate-migrate
        │              │             │               │
        │     ┌────────┼─────────┐   │               │
        │     ▼        ▼         ▼   ▼               ▼
        │ pulsate-acme pulsate-cluster pulsate-config   pulsate-http ─▶ pulsate-tls ─▶ pulsate-acme
        │     │           │         │               │             │
        │     │           │         ▼               ▼             │
        │     │           │     pulsate-flow      pulsate-pipeline ───┤
        │     │           │                         │             │
        ▼     ▼           ▼                         ▼             ▼
     pulsate-secrets   pulsate-router  pulsate-proxy  pulsate-cache   pulsate-waf
        │                  │           │            │             │
        └──────────────────┴─────┬─────┴────────────┴─────────────┘
                                  ▼
                            pulsate-core ──▶ pulsate-rt ──▶ pulsate-util
                                  ▲
                          pulsate-observe (used by ~all)
                          pulsate-plugin ──▶ pulsate-sdk (host loads guest ABI)
```

Layering rules enforced by `cargo-deny` and an `xtask` lint:
- `pulsate-core`, `pulsate-rt`, `pulsate-util` depend on nothing internal (leaves).
- Data-plane crates may depend on shared + each other, **never** on control-plane crates.
- The binary is the only crate allowed to depend on everything.

## Coding standards

- **Edition 2021**, `#![forbid(unsafe_code)]` by default; `unsafe` is permitted only in a small set of clearly-justified, reviewed, and test-covered modules (buffer management, FFI-free SIMD), each gated behind `// SAFETY:` comments and isolated.
- **No panics on request paths.** `unwrap`, `expect`, `panic!`, `todo!`, and array-index panics are denied by `clippy` on data-plane crates. Fallible paths return `Result<_, PulsateError>`.
- **Lints.** `#![warn(clippy::all, clippy::pedantic)]` workspace-wide with a curated allow-list; `rustfmt` enforced; `cargo doc` warnings are errors (docs must build).
- **Errors** use the `pulsate-core` taxonomy (thiserror-style enums internally; `anyhow`-style only in the binary/CLI top level).
- **Async hygiene.** No blocking calls in async fns (enforced by a lint + review). Every `await` point in the data plane is cancellation-safe or documented otherwise.
- **Public API discipline.** Anything `pub` in `pulsate-core`/`pulsate-sdk` requires a doc comment and a doc-test/example. `#[non_exhaustive]` on public enums/structs that may grow.
- **Comments explain *why*,** not *what*; match the density of surrounding code.

## Naming conventions

- **Crates:** `pulsate-<area>`, lower-kebab. **Binary:** `p8`.
- **Types:** `CamelCase`; traits are nouns or capabilities (`Middleware`, `CacheStore`); errors end in `Error`; config models end in `Config` (`RouteConfig`).
- **Functions/vars:** `snake_case`; constructors `new`/`with_*`/`from_*`; builders return `Self`.
- **Config keywords** (Flow): lower_snake, verbs for actions (`proxy`, `redirect`), nouns for blocks (`site`, `upstream`), consistent units (`30s`, `10MB`).
- **Metrics:** `pulsate_<subsystem>_<name>_<unit>` (Prometheus convention — see [26. Metrics Catalog](26-metrics-and-slo-catalog.md)).
- **Error codes:** `PLS-<AREA>-<NNNN>` (stable, documented — see [25. Error Catalog](25-error-and-status-catalog.md)).
- **Feature flags:** `cargo` features lower-kebab (`redis`, `vault`, `fips`).

## Testing strategy

A test pyramid mapped to crates (full detail in [28. Testing & Conformance](28-testing-and-conformance.md)):

| Level | What | Where | Gate |
|---|---|---|---|
| Unit | Pure logic: parser, matchers, cache freshness math, LB algorithms | in-crate `#[cfg(test)]` | every PR |
| Property | Invariants over random inputs (parser round-trips, router determinism) | proptest in relevant crates | every PR |
| Fuzz | Adversarial inputs: Flow parser, HTTP decoder, WAF rules | `cargo-fuzz` targets | nightly + pre-release |
| Integration | Real listeners + fake upstreams via `pulsate-test` loopback harness | `tests/` per data-plane crate | every PR |
| Conformance | h2spec, h3spec, HTTP semantics, TLS interop, ACME staging | dedicated suite | nightly + pre-release |
| Load/soak | Throughput, latency, memory under sustained load and reloads | `xtask bench` + CI perf job | nightly, release |
| Chaos | Kill upstreams, drop packets, reload under load, fill disk | scripted scenarios | pre-release |
| E2E | `p8 up` real scenarios (auto-TLS via Pebble, dashboard) | `examples/` driven | pre-release |

Targets: ≥85% line coverage on core/data-plane crates; 100% of error codes exercised; zero known-correctness regressions gate a release.

## CI/CD architecture

A pipeline organized as fast-fail stages:

```
PR  ▶ fmt + clippy + deny ▶ build (all crates, all features matrix) ▶ unit+property+integration
    ▶ doc build ▶ MSRV check ▶ minimal-versions check
    ▶ (label-gated) fuzz smoke, conformance, perf-diff
main ▶ full conformance + soak (nightly) ▶ artifact build (cross-targets) ▶ SBOM + sign
tag  ▶ release pipeline (see Release strategy)
```

- **Matrix:** Linux (x86_64, aarch64), macOS (aarch64), Windows (x86_64); stable + MSRV; feature combinations (default, `redis`, `vault`, `fips`).
- **Caching:** `sccache`/registry cache for fast PRs; per-crate incremental.
- **Quality gates:** clippy/fmt/deny must pass; coverage must not drop; a perf-diff job flags >X% regression on the benchmark suite and comments on the PR.
- **Supply chain:** `cargo-deny` (licenses/advisories/bans), `cargo-audit`, dependency review on PRs (detail in [33. Release Engineering & Supply Chain](33-release-engineering-and-supply-chain.md)).

## Release strategy

- **Channels:** `nightly` (every main commit, unsigned-but-attested dev builds), `beta` (pre-release hardening), `stable` (tagged, signed, supported).
- **Artifacts:** static binaries per target, container images (distroless), OS packages (`.deb`/`.rpm`), a Homebrew formula, and checksums + signatures (Sigstore/cosign) + SBOM (CycloneDX) for every artifact.
- **Reproducible builds** so a third party can verify a binary matches the tagged source.
- **Release notes** are generated from Conventional Commits, with a human-curated "highlights" and an explicit "breaking changes / migration" section.
- **Cadence:** time-boxed minor releases (e.g., every 6–8 weeks) plus out-of-band patch releases for security fixes per the [18. Open Source](18-open-source.md) security policy.

## Versioning

- **SemVer** for the binary and the public crates (`pulsate-core`, `pulsate-sdk`).
- **Pre-1.0 (0.x):** breaking changes allowed at minor bumps but always documented with migrations and confined to scheduled windows; the config format is the first thing stabilized.
- **Config format versioning is independent** via `flow_version "1"` at the top of `pulsate.flow`. A binary supports a range of `flow_version`s and warns on deprecation, so upgrading the binary never silently breaks a config.
- **Plugin ABI versioning is independent** again (WIT world version), so plugins keep working across binary upgrades within an ABI major (see [12. Plugins](12-plugins.md)).
- **MSRV** policy: latest stable minus two releases; MSRV bumps are a minor-version event, announced in release notes.

## Cross-references
- [02. Architecture](02-architecture.md) — the layering the crate graph enforces.
- [12. Plugins](12-plugins.md) — `pulsate-plugin`/`pulsate-sdk` and ABI versioning.
- [28. Testing & Conformance](28-testing-and-conformance.md) — the deep test plan.
- [33. Release Engineering & Supply Chain](33-release-engineering-and-supply-chain.md) — signing, SBOM, reproducibility.
- [18. Open Source](18-open-source.md) — governance, contribution, security policy.
