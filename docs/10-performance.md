# 10. Performance

> Performance as an engineered, measured property: explicit throughput/latency/memory/scalability goals, the benchmark methodology that holds us to them, and the techniques (lock-free structures, allocation strategy, SIMD, kernel offloads) that get us there.

**Contents**
- [Performance goals](#performance-goals)
- [Memory goals](#memory-goals)
- [Latency goals](#latency-goals)
- [Scalability goals](#scalability-goals)
- [Benchmark methodology](#benchmark-methodology)
- [Profiling](#profiling)
- [Lock-free & low-contention structures](#lock-free--low-contention-structures)
- [Memory allocation strategy](#memory-allocation-strategy)
- [SIMD opportunities](#simd-opportunities)
- [Kernel optimizations](#kernel-optimizations)
- [Cross-references](#cross-references)

---

## Performance goals

Goals are **targets the benchmark suite enforces**, not marketing numbers. They are stated as ranges on reference hardware (defined in [Benchmark methodology](#benchmark-methodology)) and tracked over time to prevent regressions.

| Goal | Target (reference hardware) | Why |
|---|---|---|
| Throughput (HTTP/1.1, keep-alive, tiny body) | ≥ ~90% of the fastest comparable Rust/C proxy | proves the architecture has no structural overhead |
| Throughput (TLS 1.3, HTTP/2) | within a small constant of cleartext on the same box | TLS cost should be crypto, not framework |
| Reverse-proxy throughput | bottlenecked by upstream/NIC, not by Pulsate | the proxy path must be near-zero-overhead |
| Cache hit serve | sub-millisecond from memory store | caching must be clearly worth it |
| Reload | zero dropped connections, sub-second snapshot swap | hot reload must be free at the edge |

The guiding principle: **on the hot path, do no allocation, take no lock, make no syscall you don't have to.** Performance is a first-class acceptance criterion in [19. Milestones](19-milestones.md).

## Memory goals

- **Bounded by configuration, not input.** Peak memory is a function of configured limits (connections × buffers, cache size, pool sizes), so an adversary cannot drive memory growth. Every buffer, queue, and map has a cap.
- **Low per-connection footprint** so a single node holds hundreds of thousands of idle keep-alive/QUIC connections (the C10M target class) within a predictable RAM budget.
- **Steady-state allocation near zero** on the request path (buffer pools + `Bytes`), so the allocator and GC-like pauses (Rust has none, but fragmentation/contention can mimic them) are non-factors.
- **No leaks under churn:** sustained soak tests (connections, reloads, cache turnover) must show flat RSS.

## Latency goals

- **Tail latency is the metric.** We optimize p99/p999, not just mean — a proxy's job is consistency. Targets: single-digit-microsecond added latency at p50 for proxied requests, and tightly-bounded p99 under load (no multi-millisecond cliffs from locks, allocation spikes, or scheduler stalls).
- **No head-of-line surprises:** HTTP/2/3 stream isolation, per-request tasks, and backpressure keep one slow request from stalling others.
- **Reload/cert-renewal/discovery never spikes request latency** because they happen off the data plane (snapshot model).

## Scalability goals

- **Vertical:** near-linear throughput scaling with cores via `SO_REUSEPORT` accept distribution and share-nothing per-task state; optional CPU pinning and (future) thread-per-core io_uring for the steepest scaling.
- **Connection scale:** efficient handling of very high concurrent connection counts (idle-cheap), important for HTTP/3/mobile and SSE/WebSocket fan-out.
- **Horizontal:** stateless data plane means adding nodes scales throughput linearly; shared state (certs, cache L2, rate-limit counters, sticky) is externalized to the cluster/Redis so nodes coordinate without a bottleneck ([16. Deployment](16-deployment.md)).

## Benchmark methodology

Credible, reproducible, and apples-to-apples — to avoid the "benchmarketing" the proxy space is full of:

- **Reference hardware** is pinned and documented (CPU model, core count, NIC, kernel version) for both a cloud-VM profile and a bare-metal profile. Results are always reported with the environment.
- **Workloads** cover the matrix in [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md): tiny vs large bodies, keep-alive vs new-conn, HTTP/1.1 vs h2 vs h3, TLS handshake rate, cache hit vs miss, and reverse-proxy vs origin-direct (to isolate Pulsate's overhead).
- **Tools:** `wrk`/`wrk2` (constant-rate, correct latency under coordinated omission), `h2load` (h2/h3), `vegeta`, and a custom harness in `pulsate-test`. Latency uses HdrHistogram; we report full percentiles, not averages.
- **Comparisons** run competitors (nginx, Caddy, Envoy, Traefik) on the *same* box with *equivalent* configs, documented so anyone can reproduce. We publish configs and raw data.
- **Regression gating:** a CI perf job runs a subset on fixed hardware and **fails/▲-comments the PR** on a >X% regression (see [03. Repository — CI/CD](03-repository.md)). Full runs happen nightly and pre-release.
- **Honesty rule:** any benchmark we publish includes the losing cases and the methodology; we never cherry-pick.

## Profiling

- **Continuous & on-demand:** `pprof`-style CPU/heap profiling exposed via the admin API (`/v1/debug/pprof`, loopback-gated) so operators can profile a live process; flamegraphs via `p8 bench --profile`.
- **Tooling:** `cargo flamegraph`, `perf`, `tokio-console` (task stalls, busy loops), `heaptrack`/`dhat` (allocations), and `bytehound` for leak hunts in CI soak.
- **Tracing for latency:** per-stage timing in the [request lifecycle](02-architecture.md#request-lifecycle) is recorded so a slow request can be attributed to a specific stage/middleware (the basis for AI-assisted diagnostics in [20. Future](20-future.md)).
- **Benchmark crates:** `criterion` microbenchmarks for parsers, matchers, LB selection, cache math — guarding the inner loops.

## Lock-free & low-contention structures

The architecture avoids locks on the hot path by construction:
- **Config:** `arc-swap` — readers never block, never lock; publish is one atomic store ([02. Architecture](02-architecture.md#memory-model)).
- **Counters & gauges:** per-shard/per-core atomics, aggregated lazily, so metric updates don't contend.
- **Sharded maps** (striped locks or lock-free) for rate-limit counters, breaker state, pool registries, and cache indexes — contention scales with shard count, not a single mutex.
- **Per-task ownership:** connection/request state is task-local; cross-task sharing is immutable or via bounded channels, eliminating most shared-mutable state outright.
- **Wait-free fast paths** where it matters (e.g., LB power-of-two-choices reads two atomic counters, no lock).

We measure contention (lock-wait, false sharing via `perf c2c`) and treat a hot lock as a bug.

## Memory allocation strategy

- **Buffer pools** (`pulsate-util`): per-worker free-lists of right-sized read/write buffers, recycled across requests; sizes adapt within caps. Result: steady-state request handling allocates ~nothing.
- **`Bytes` everywhere** for bodies/headers: refcounted, slice-able, cheap to clone — bytes are moved by pointer, not copied, through middleware and the proxy.
- **Small-vector / inline storage** for header maps and short collections to avoid heap traffic in the common case.
- **Arena/bump allocation** for per-request scratch where lifetimes are clearly request-scoped.
- **Global allocator:** mimalloc (or jemalloc per platform) in release builds for low fragmentation and good multi-threaded behavior; chosen by benchmarking, not folklore.
- **Backpressure over buffering:** when in doubt, slow the producer rather than grow a buffer (bounded everything).

## SIMD opportunities

Applied where data-parallel and proven beneficial:
- **HTTP header parsing / token scanning** — SIMD-accelerated find/validate of header delimiters and illegal bytes (the kind of speedups behind fast HTTP parsers); used in `pulsate-http`'s codec hot path.
- **WAF/rule matching** — vectorized multi-pattern search (e.g., Aho-Corasick with SIMD) for scanning request data against many signatures.
- **Compression/hashing** — rely on SIMD-optimized libraries (zstd/brotli, BLAKE3 for ETag/content hashing).
- **TLS** — handled by rustls + its crypto provider, which use platform SIMD/AES-NI.
- Portability via `std::simd`/`portable-simd` with scalar fallbacks; never a correctness compromise, always behind a measured win and runtime feature detection.

## Kernel optimizations

Linux-first, with graceful fallbacks elsewhere:
- **`SO_REUSEPORT`** for multi-queue accept across cores/workers without an accept lock.
- **`sendfile`/`splice`** for zero-copy static file serving (cleartext); **kTLS** to extend zero-copy to TLS where the kernel/NIC supports it.
- **io_uring** as a future `pulsate-rt` data-plane backend (batched, syscall-light, thread-per-core) — the biggest pending lever for tail latency and throughput on Linux.
- **GSO/GRO + `sendmmsg`/`recvmmsg`** for high-throughput UDP/QUIC.
- **TCP tuning** surfaced as config/guidance: `TCP_NODELAY`, deferred accept, backlog sizing, buffer sizes, and recommended sysctls (fd limits, ephemeral port range, `somaxconn`) documented in [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md).
- **NUMA awareness** (pinning + local allocation) for large bare-metal boxes.

Every kernel optimization degrades safely: if the OS doesn't support it, Pulsate falls back to the portable path, so the one-binary promise holds across platforms.

## Cross-references
- [02. Architecture](02-architecture.md) — snapshot/lock-free model, buffer/memory model, runtime.
- [05. HTTP Stack](05-http-stack.md) — zero-copy, buffer management, io_uring/kTLS.
- [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md) — the full benchmark matrix and tuning guide.
- [28. Testing & Conformance](28-testing-and-conformance.md) — load/soak/chaos as release gates.
- [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — latency/throughput SLIs and SLOs.
