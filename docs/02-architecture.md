# 02. Architecture

> The system from the outside in: how Pulsate is structured into a control plane and a data plane, how a request flows through ten named lifecycle stages, how configuration becomes an atomically-swapped snapshot, and how the pieces talk without locks on the hot path.

**Contents**
- [Overall system architecture](#overall-system-architecture)
- [Control plane vs data plane](#control-plane-vs-data-plane)
- [Process model](#process-model)
- [Thread model](#thread-model)
- [Async runtime](#async-runtime)
- [Connection lifecycle](#connection-lifecycle)
- [Request lifecycle](#request-lifecycle)
- [Memory model](#memory-model)
- [Configuration loading](#configuration-loading)
- [Hot reload architecture](#hot-reload-architecture)
- [Graceful shutdown](#graceful-shutdown)
- [Worker lifecycle](#worker-lifecycle)
- [Error handling](#error-handling)
- [Dependency injection strategy](#dependency-injection-strategy)
- [Module communication](#module-communication)
- [Internal APIs](#internal-apis)
- [Extension points](#extension-points)
- [Cross-references](#cross-references)

---

## Overall system architecture

Pulsate is a single process split into two cooperating halves: a **data plane** that moves bytes on the hot path, and a **control plane** that decides policy off the hot path. They are joined by exactly one shared object — an immutable **`ConfigSnapshot`** — which the control plane publishes and the data plane reads lock-free.

```
                          ┌──────────────────────────── PULSE PROCESS ───────────────────────────┐
                          │                                                                       │
   pulsate.flow  ──────────▶│  ┌───────────────── CONTROL PLANE ─────────────────┐                 │
   admin API   ──────────▶│  │ Config loader → Flow parser → validator         │                 │
   K8s/CRD     ──────────▶│  │ Snapshot builder ─────────────┐                 │                 │
   file watch  ──────────▶│  │ ACME/cert manager             │ publishes       │                 │
                          │  │ Cluster coordinator           ▼                 │                 │
                          │  │ Admin API + Dashboard   ┌─────────────────┐     │                 │
                          │  │ Metrics aggregator      │  arc-swap<Arc<  │◀────┘                 │
                          │  └─────────────────────────│  ConfigSnapshot>│  (atomic publish)      │
                          │                            └────────┬────────┘                        │
                          │                                     │ lock-free read                  │
                          │  ┌───────────────── DATA PLANE ─────┼───────────────────────────────┐ │
   client ──TCP/UDP──────▶│  │ Listeners → TLS/QUIC → HTTP codec│→ Router → Pipeline → Handler   │ │
   (h1/h2/h3)             │  │   (pulsate-net) (pulsate-tls)(pulsate-http/3)(pulsate-router)(pulsate-      │ │
                          │  │                                  │         pipeline) (pulsate-proxy)│ │
                          │  │                                  └──▶ Cache / WAF / Upstream pools │ │
                          │  └────────────────────────────────────────────────────────────────┘ │
                          │           │ metrics/traces/logs → pulsate-observe → exporters            │
                          └───────────────────────────────────────────────────────────────────────┘
```

The design rule that everything else follows: **the data plane never blocks on the control plane.** Configuration changes, certificate renewals, and cluster events are all reduced to "build a new immutable snapshot and swap the pointer." A request in flight holds an `Arc` to the snapshot it started with and runs to completion against it, even while a newer snapshot is already live.

## Control plane vs data plane

| | **Data plane** | **Control plane** |
|---|---|---|
| Job | Serve requests at line rate | Decide configuration, manage lifecycle |
| Latency budget | Microseconds; on the critical path | Milliseconds–seconds; off the path |
| Concurrency | Massive (per-connection tasks) | Small (a handful of long-lived tasks) |
| State | Reads immutable `ConfigSnapshot` | Owns mutable sources of truth |
| Crates | `pulsate-net/tls/http/http3/router/pipeline/proxy/cache/waf` | `pulsate-control/config/flow/acme/cluster/secrets/dashboard` |
| Failure posture | Must degrade gracefully, never panic a connection into the others | May retry, log, alert; isolated from request serving |

The split is *in-process*, which is the key departure from Envoy's external xDS control plane. We get the engineering benefits of the split (testability, a clear policy/mechanism boundary, safe reloads) without the operational cost of running and securing a second distributed system. For multi-node fleets, an *optional external* control plane is a future layer ([20. Future](20-future.md)), but a single Pulsate is always complete on its own.

**Boundary contract.** The only types that cross the boundary are: `ConfigSnapshot` (control → data, via `arc-swap`), control `Command`s and `Event`s (via channels, see [Module communication](#module-communication)), and metric samples (data → observe). No data-plane code calls into the control plane synchronously.

## Process model

- **Single process by default.** A supervisor task spawns and owns: listener tasks (one set per bound socket), the control-plane tasks, the observability exporter, and the admin/dashboard server. This is the simplest correct model and matches "one binary."
- **Optional prefork workers** (`pulsate up --workers N`, or `pulsate { workers N }`). Each worker is a forked process that binds the same ports via `SO_REUSEPORT`, letting the kernel load-balance accepted connections across workers. Workers share nothing except the listening sockets; each builds its own snapshot from the same config. Rationale: lets Pulsate scale past a single runtime's limits and isolates worker crashes, without a threading model that shares mutable request state.
- **Privilege handling.** Pulsate may bind privileged ports (80/443) and then **drop privileges** to a configured unprivileged user/group before serving (`pulsate { user "pulsate"; group "pulsate" }`). On Linux, `CAP_NET_BIND_SERVICE` is the preferred alternative to running as root at all.
- **Supervision.** The supervisor restarts a crashed listener task with capped exponential backoff. A worker process that dies is respawned by the supervisor (single-process mode) or by the parent (prefork mode). Repeated immediate crashes trip a circuit and surface a fatal diagnostic rather than crash-looping.

## Thread model

- The default runtime is a **Tokio multi-threaded scheduler** sized to available cores (overridable via `pulsate { runtime { worker_threads N } }`).
- **Accept scaling** uses `SO_REUSEPORT`: each runtime worker (or prefork process) owns an accept loop on its own socket clone, so accepts spread across cores without a single accept mutex.
- **CPU affinity** is optional (`runtime { pin_workers true }`) for latency-sensitive deployments, pinning runtime worker threads to cores to improve cache locality.
- **Blocking work is banished from the hot path.** Anything potentially blocking (disk cache writes, DNS without the async resolver, filesystem stat) runs on Tokio's blocking pool or a dedicated thread, never inline in a request task.
- The data plane is **share-nothing per task** wherever possible. Per-connection and per-request state lives on the task's stack/heap; cross-task shared state is immutable (the snapshot) or explicitly concurrent (atomics, sharded maps — see [Memory model](#memory-model)).

## Async runtime

Tokio is the chosen runtime for its maturity, ecosystem (hyper, quinn, rustls integrations), and work-stealing scheduler. The decision and its alternative (a thread-per-core io_uring runtime) are recorded in [ADR](24-architecture-decision-records.md).

To avoid betting the whole codebase on one runtime, the data plane depends on **`pulsate-rt`**, a thin runtime-abstraction crate exposing the primitives the data plane actually uses: `spawn`, `spawn_blocking`, timers, TCP/UDP listeners and streams, and a yield point. Today `pulsate-rt` is a Tokio adapter. This keeps open a future **thread-per-core backend** (monoio/glommio + io_uring) for the data plane on Linux — a per-core, share-nothing model that can materially cut tail latency — without rewriting `pulsate-http`, `pulsate-proxy`, etc. The control plane always uses Tokio directly (it is not latency-critical).

## Connection lifecycle

A connection is owned by exactly one task for its entire life.

```
accept() ─▶ [Accept] ─▶ TLS/QUIC handshake [Handshake] ─▶ protocol select (ALPN)
   │                                                              │
   │                                          ┌───────────────────┼───────────────────┐
   │                                       HTTP/1.1            HTTP/2               HTTP/3
   │                                     (1 req/conn,        (N streams,          (N streams,
   │                                      keep-alive)         multiplexed)         over QUIC)
   │                                          │                   │                   │
   │                                          └──────── per request: Request lifecycle ┘
   │                                                              │
   └── idle keep-alive (timeout) ─── connection close / drain ────┘
```

- **Accept.** The listener task accepts a socket, applies socket options (TCP_NODELAY, keepalive, buffer sizes), and enforces global/per-IP connection limits (an early WAF gate, see [09. Security](09-security.md)).
- **Handshake.** For TLS, `pulsate-tls` (rustls) performs the handshake, selecting the certificate by SNI from the snapshot's cert view and negotiating the protocol via ALPN (`h2`, `http/1.1`). For HTTP/3, `pulsate-http3` runs the QUIC handshake via quinn.
- **Multiplexing.** HTTP/1.1 serves one request at a time with keep-alive; HTTP/2 and HTTP/3 fan out into concurrent per-stream request tasks with flow control and configurable concurrency caps.
- **Idle & lifetime limits.** Idle keep-alive timeout, max requests per connection, and max connection lifetime are all bounded to prevent resource pinning. On shutdown or reload-driven listener change, connections drain (see [Graceful shutdown](#graceful-shutdown)).

## Request lifecycle

Every HTTP request passes through the **ten canonical stages** below. These names are normative — every other document (middleware, proxy, observability, errors) refers to them.

```
 [1 Accept] ─ connection established
 [2 Handshake] ─ TLS/ALPN or QUIC negotiated
 [3 Decode] ─ request head parsed into a normalized Request
 [4 Match] ─ Router resolves site + route from the snapshot
 [5 Ingress] ─ request-phase middleware run in declared order ──┐
 [6 Dispatch] ─ terminal Handler chosen (proxy/files/redirect)  │ any stage may divert to
 [7 Upstream] ─ (proxy) pool pick, connect, send, receive       │      ↓
 [8 Egress] ─ response-phase middleware run in REVERSE order    │  [Recover] error middleware
 [9 Stream] ─ response body streamed to client                  │  → synthesizes a response,
 [10 Finalize] ─ access log, metrics, trace span closed ────────┘    re-enters at [8 Egress]
```

- **[3] Decode** normalizes the request into a `Request` carrying method, URI, version, headers, and a streaming body handle. Header limits and malformed-input rejection happen here.
- **[4] Match** consults the routing table inside the current `ConfigSnapshot`: host match → route match (path/regex/method/header predicates) → the route's middleware list and handler. Matching is allocation-free and deterministic (see [06. Reverse Proxy](06-reverse-proxy.md)).
- **[5] Ingress / [8] Egress** are the two halves of the middleware pipeline. Ingress runs in declared (left-to-right, `~>`) order and may short-circuit (e.g., a rate limiter returning 429 skips straight to Egress). Egress runs the *same* middleware in reverse so that wrapping concerns (compression, headers, tracing) nest correctly. (See [07. Middleware](07-middleware.md).)
- **[6] Dispatch / [7] Upstream** invoke the terminal handler. For a reverse-proxy handler, Upstream covers pool selection, connection reuse, request forwarding, retries, and circuit-breaking (see [06. Reverse Proxy](06-reverse-proxy.md)).
- **[9] Stream** pushes the response body to the client with backpressure; bodies are never fully buffered unless a middleware explicitly requires it (e.g., a transform).
- **[10] Finalize** is guaranteed to run exactly once per request (even on error) and is where the access log line, metrics, and trace span are emitted.
- **[Recover]** is the error path: any stage that yields an `Error` hands control to the Recover phase, which maps the error to a response (status, problem+json body, headers) per the [25. Error Catalog](25-error-and-status-catalog.md), then resumes at Egress so that response middleware still apply.

A single immutable value, the **`RequestCtx`**, threads through all stages carrying the request, the response-in-progress, the matched route, the snapshot `Arc`, request-scoped key/value extensions (a typed map), timing, and the request ID. Middleware and handlers receive `&mut RequestCtx`.

## Memory model

- **Immutable shared state via `Arc`.** The `ConfigSnapshot` (and everything reachable from it: routes, handler configs, the cert view) is immutable and shared by `Arc`. Reads are pointer-chases, never locks.
- **`arc-swap` for publication.** The live snapshot lives in an `ArcSwap<ConfigSnapshot>`. Publishing a new config is a single atomic store; reading is a lock-free load that hands the task an `Arc` it owns for the request's duration.
- **Sharded concurrent maps** (e.g., `dashmap` or a custom striped map) for mutable hot-path state that *cannot* be snapshot-immutable: rate-limit counters, circuit-breaker state, connection-pool registries, cache indexes. Sharding avoids a global lock.
- **Buffer pooling.** Read/write buffers are pooled and reused per worker to avoid per-request allocation churn (see [05. HTTP Stack](05-http-stack.md) — buffer management). Bodies use `Bytes` (refcounted, cheaply cloned, zero-copy slices).
- **Bounded everything.** Header sizes, body buffer caps, in-flight request counts, pool sizes, and cache sizes are all bounded so memory is a function of configured limits, not of adversarial input. (See [10. Performance](10-performance.md).)
- **Allocator.** The release build uses a high-performance allocator (mimalloc or jemalloc, selected per platform) to reduce fragmentation and contention under high concurrency.

## Configuration loading

Loading is a pure pipeline from source text to a validated snapshot (owned entirely by the control plane):

```
source (file | admin API | CRD | env) 
   → [pulsate-flow] lex → parse → AST 
   → [pulsate-config] resolve includes/env/secrets → typed Config model 
   → validate (schema + cross-references + invariants) 
   → build ConfigSnapshot (compile routing table, resolve upstreams, bind cert requirements) 
   → publish via ArcSwap
```

- **Sources** are pluggable: the primary file `pulsate.flow`, the admin API (`PUT /config`), a Kubernetes CRD/Gateway-API watcher, and environment overlays. All sources produce the same `Config` model.
- **Validation is total before publish.** A config that fails validation never becomes a snapshot; the running snapshot is untouched. Validation includes schema (types/required), referential integrity (every `@upstream` exists, every plugin is loaded), and invariants (no two sites claim the same host+port, TLS automation has a reachable challenge path).
- **Determinism.** Building a snapshot from the same `Config` always yields an equivalent routing table. Snapshots are content-addressable (hashed) for diffing and audit (see [22. Admin API](22-admin-api.md), [23. Data & State Model](23-data-and-state-model.md)).

## Hot reload architecture

Reloads are **snapshot swaps**, not process restarts. There is no fork-and-drain of the whole server, and no dropped connections.

```
new config arrives ─▶ parse+validate (control plane) ─▶ build new ConfigSnapshot
        │                                                        │
   fail │ keep old snapshot, return errors to caller             │ success
        ▼                                                        ▼
   reload rejected (running traffic untouched)        ArcSwap.store(new_snapshot)
                                                                 │
        ┌────────────────────────────────────────────────────────┘
        ▼
 In-flight requests: keep their old Arc<Snapshot>, run to completion.
 New requests: load() returns the new snapshot.
 Listeners: only sockets whose bind/TLS changed are reconciled; unchanged listeners keep running.
 Resources: upstream pools, cache, and cert store are keyed so unchanged entries are reused, not rebuilt.
```

- **Listener reconciliation.** The reload diffs old vs new listener sets. Sockets that are unchanged keep serving; sockets that are added are bound; sockets that are removed are drained and closed. This means a reload that only changes a route never touches the listening sockets at all.
- **Resource carry-over.** Upstream connection pools, the cache, and issued certificates are owned in registries keyed by identity (e.g., upstream name+target). The new snapshot references the same live pool object when the upstream is unchanged, so reloads do not cold-start connection pools or evict the cache.
- **Triggers.** A reload can be triggered by `pulsate reload` (signals the running process), a `SIGHUP`, a file-watch on `pulsate.flow` (`--watch`), or an admin-API call. All funnel into the same validate-build-swap path.
- **Atomicity & rollback.** The swap is atomic; if a newly published snapshot causes elevated errors within a guard window, the control plane can auto-rollback to the previous snapshot (kept for one generation) — an operator-configurable safety net.

## Graceful shutdown

```
signal (SIGTERM / `pulsate down`) 
  ▶ stop accepting new connections (close listener accept loops)
  ▶ broadcast Drain to all connection tasks
  ▶ HTTP/1.1: finish in-flight request, send `Connection: close`
     HTTP/2/3: send GOAWAY, let active streams finish, refuse new streams
  ▶ wait up to `shutdown.grace` (default 30s) for connections to drain
  ▶ force-close stragglers past the deadline
  ▶ flush access logs, metrics, traces; persist cache index & state
  ▶ release sockets; exit 0
```

Shutdown is driven by a single broadcast `watch::Receiver<Lifecycle>` that every task observes. The grace deadline is bounded so shutdown always terminates. For zero-downtime upgrades, prefork mode supports socket handoff so a new binary inherits the listening sockets while the old workers drain (see [16. Deployment](16-deployment.md)).

## Worker lifecycle

A "worker" is a unit that owns request serving — either a runtime worker thread (single-process mode) or a prefork process (multi-process mode). Each worker:

```
spawn ─▶ build snapshot from current Config ─▶ bind/inherit sockets ─▶ enter accept+serve loop
   ▲                                                                          │
   │                                                  reload: load new snapshot (no restart)
   │                                                                          │
   └──────────── supervisor restarts on crash (backoff) ◀── panic/exit ───────┘
```

- Workers are **stateless with respect to each other** — no shared mutable request state — so a crashed worker is replaced without coordinating with peers.
- **Health.** Each worker reports liveness/readiness to the supervisor and to the admin API; readiness gates load-balancer traffic during startup and drain.
- **Backpressure.** A worker nearing its in-flight or memory limits sheds load (503 with `Retry-After`) rather than accepting work it cannot serve — explicit, observable degradation.

## Error handling

Pulsate uses a **layered, typed error strategy** (full taxonomy in [25. Error Catalog](25-error-and-status-catalog.md)):

- **`pulsate-core` defines the error types.** Library crates return `Result<T, PulsateError>` with rich, structured variants (a stable `code`, a category, a message, and optional source). No `unwrap`/`expect`/`panic!` on any request path — enforced by lint and code review (see [03. Repository](03-repository.md)).
- **Errors are values, not control flow.** A request-path error becomes a `Response` via the Recover phase; it never unwinds across the connection boundary. A panic in a request task is caught at the task boundary, logged with the request ID, converted to a 500, and isolated to that request — never the whole worker.
- **Config errors** are reported with file/line/column and a suggested fix at load time, and never affect the running snapshot.
- **Operational errors** (upstream down, cert renewal failed) are surfaced as metrics, structured logs, and admin-API status, with the request-time behavior (retry, fail-open/closed) governed by config.
- **Exit codes** are stable and documented for scripting (validation failure, bind failure, fatal config error — see [13. CLI](13-cli.md), [25. Error Catalog](25-error-and-status-catalog.md)).

## Dependency injection strategy

Pulsate avoids a runtime DI container/framework (they obscure control flow and cost startup time). Instead it uses **compile-time, constructor-based composition with trait objects at the seams**:

- **Traits define seams.** `Middleware`, `Handler`, `Matcher`, `Upstream`, `CacheStore`, `SecretsBackend`, `CertStore`, and `MetricsSink` are traits in `pulsate-core`. Concrete implementations live in their crates.
- **A `Registry` resolves names to constructors.** At snapshot-build time, the control plane consults a `Registry` (a typed map from a config keyword like `compress` or `redis` to a factory) to instantiate the objects a config references. Built-ins register at startup; plugins register their factories when loaded (see [Extension points](#extension-points)).
- **The `RequestCtx` is the injection vehicle on the hot path.** Anything a middleware/handler needs (the snapshot, shared services, request-scoped extensions) is reached through `RequestCtx`, not through globals. There is no ambient global state.
- **`Arc<dyn Trait>` for shared services, generics where monomorphization matters.** Cross-cutting services (cache, metrics) are shared as `Arc<dyn _>`. The very hottest inner loops use generics/static dispatch to avoid vtable cost where profiling shows it matters.

The result is explicit wiring: you can read the constructor of each subsystem and see exactly what it depends on, and tests inject fakes by passing different implementations — no magic.

## Module communication

| Channel | Direction | Mechanism | Purpose |
|---|---|---|---|
| Config publish | control → data | `ArcSwap<Arc<ConfigSnapshot>>` | lock-free live config |
| Lifecycle | supervisor → all | `watch::Receiver<Lifecycle>` | start/drain/shutdown broadcast |
| Commands | admin/CLI → control | `mpsc<Command>` | reload, purge cache, issue cert, etc. |
| Events | any → observe/control | `broadcast<Event>` | cert issued, upstream down, reload done |
| Metrics | data → observe | non-blocking `metrics` facade | counters/histograms with bounded cardinality |
| Cluster | node ↔ node | gossip/RPC (`pulsate-cluster`) | membership, shared cache/state |

The rules: **no synchronous data→control calls on the request path**; cross-task communication is via channels or the snapshot; and every channel is bounded so a slow consumer applies backpressure rather than growing unboundedly.

## Internal APIs

Three stability tiers govern internal interfaces:

1. **Public Rust API** (`pulsate-core`, `pulsate-sdk`) — semver-stable traits and types that plugin authors and embedders depend on. Changes follow the deprecation policy in [03. Repository](03-repository.md).
2. **Plugin host ABI** — the WIT-defined boundary between Pulsate and WASM plugins, versioned independently (see [12. Plugins](12-plugins.md)). This is the most carefully guarded contract.
3. **Crate-internal APIs** — interfaces between Pulsate's own crates. Not stability-guaranteed for external consumers, but governed by clear ownership and tested at the crate boundary.

The **Admin API** (HTTP/gRPC over the control plane) is the external runtime control surface and is fully specified in [22. Admin API](22-admin-api.md); it is versioned (`/v1/...`) with its own stability guarantees.

## Extension points

Pulsate is extensible at well-defined seams, in increasing order of power:

- **Config-level composition** — combine built-in middleware and handlers in `pulsate.flow` (no code).
- **WASM plugins** — add middleware, handlers, matchers, auth providers, and cache/secrets backends in any language that compiles to a WASM component, against the stable host ABI, fully sandboxed ([12. Plugins](12-plugins.md)).
- **Native Rust extensions** — for embedders building atop the crates, the same `pulsate-core` traits can be implemented in-tree (the path for built-ins, and for performance-critical extensions where WASM overhead is unacceptable).
- **Service discovery & secrets providers** — pluggable `SecretsBackend`/discovery implementations register through the `Registry`.
- **Observability sinks** — custom `MetricsSink`/trace exporters via the standard OpenTelemetry/Prometheus interfaces ([15. Observability](15-observability.md)).

Every extension point is a `pulsate-core` trait plus a `Registry` registration, so "adding a feature" and "loading a plugin" are the same mechanism viewed from two sides.

## Cross-references
- [04. Configuration](04-configuration.md) — the Flow source that becomes a snapshot.
- [05. HTTP Stack](05-http-stack.md) — Decode/Stream/buffer details for Handshake & protocols.
- [06. Reverse Proxy](06-reverse-proxy.md) — Match, Dispatch, and Upstream stages in depth.
- [07. Middleware](07-middleware.md) — the Ingress/Egress/Recover pipeline.
- [22. Admin API](22-admin-api.md) & [23. Data & State Model](23-data-and-state-model.md) — control-plane surface and state.
- [24. ADRs](24-architecture-decision-records.md) — the recorded rationale for runtime, snapshot, and split decisions.
- [25. Error Catalog](25-error-and-status-catalog.md) — the Recover phase and error taxonomy.
