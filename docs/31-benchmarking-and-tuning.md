# 31. Benchmarking and Tuning

> The reproducible benchmark matrix that holds Pulsate to its performance goals, and the operator tuning guide for getting the most out of a deployment. Methodology over marketing.

**Contents**
- [Benchmark principles](#benchmark-principles)
- [Reference environments](#reference-environments)
- [Workload matrix](#workload-matrix)
- [Tools & measurement](#tools--measurement)
- [Comparison methodology](#comparison-methodology)
- [Reporting](#reporting)
- [Operator tuning guide](#operator-tuning-guide)
- [Cross-references](#cross-references)

---

## Benchmark principles

The proxy space is full of misleading benchmarks. Pulsate commits to:
1. **Reproducibility** — every number ships with the hardware, OS, kernel, configs, command lines, and raw data so anyone can re-run it.
2. **Correct latency** — constant-rate (open-model) load with coordinated-omission correction and full percentiles (HdrHistogram); never "average latency at max throughput."
3. **Isolate Pulsate's overhead** — always measure origin-direct vs through-Pulsate on the same box to report *added* latency, not the backend's.
4. **Show the losses** — published comparisons include the cases where Pulsate is not fastest, with analysis.
These tie back to the goals and SLIs in [10. Performance](10-performance.md).

## Reference environments

Two pinned profiles, both documented to the kernel version:
- **Cloud VM profile:** a defined instance type (e.g., 8 vCPU, 16 GB, 10 Gbps), representative of typical deployments.
- **Bare-metal profile:** a defined multi-core box (e.g., 32 cores, fast NIC) for ceiling/scaling tests, including NUMA.
Load generators run on **separate** machines over a low-latency link so the generator isn't the bottleneck; the network path is characterized (baseline RTT, max pps).

## Workload matrix

| Axis | Variants |
|---|---|
| Protocol | HTTP/1.1, HTTP/2, HTTP/3 |
| Security | cleartext, TLS 1.3 (new handshake rate + resumed) |
| Body size | 0 B, 1 KB, 10 KB, 100 KB, 1 MB, 10 MB |
| Connection | keep-alive (reuse) vs new-connection-per-request |
| Mode | static file serve, reverse proxy, cache hit, cache miss |
| Concurrency | sweep (e.g., 50 → 50k connections) |
| Mix | synthetic + a "realistic" blended profile (mixed sizes/routes) |

Each cell reports throughput (rps, MB/s), latency percentiles (p50/p90/p99/p999), CPU and RSS, and (for TLS) handshakes/sec.

Key derived metrics:
- **Proxy overhead** = through-Pulsate latency − origin-direct latency (the number that matters most).
- **TLS cost** = TLS throughput / cleartext throughput on the same workload.
- **Scaling efficiency** = throughput(N cores) / (N × throughput(1 core)).

## Tools & measurement

- **`wrk2`** — constant-rate HTTP/1.1/2 with correct latency under coordinated omission.
- **`h2load`** — HTTP/2 and HTTP/3 throughput/latency.
- **`vegeta`** — scripted, constant-rate, good for mixed profiles and reports.
- **`pulsate bench`** — the built-in generator (HdrHistogram, CO-aware) for quick local runs ([13. CLI](13-cli.md)).
- **Server-side:** per-stage metrics ([26. Metrics Catalog](26-metrics-and-slo-catalog.md)), `perf`/flamegraphs, `tokio-console`, allocation profiling — to attribute results to code, not guess.
Warm-up periods are discarded; runs are repeated and reported with variance; outlier runs are investigated, not dropped.

## Comparison methodology

To compare against nginx, Caddy, Envoy, and Traefik fairly:
- **Same box, same backend, same workload, equivalent config** — feature parity matched (e.g., all do TLS 1.3 + keep-alive + the same routing), with each competitor's config tuned per its own best-practice docs (not strawman defaults).
- **Same measurement harness** for all (one generator, one methodology).
- **Publish all configs and raw data** in a benchmarks repo; invite reproduction and corrections.
- Report **per-workload winners** — different tools win different cells; we present the full grid, not a single headline.

## Reporting

- A versioned **benchmarks report** per release: tables + percentile plots + flamegraphs + the exact environment and commands.
- A **regression dashboard:** the CI perf-gate runs a subset on fixed hardware every PR and tracks trends; a >X% regression flags/▲-comments the PR ([03. Repository](03-repository.md)).
- **No cherry-picking** policy stated up front; methodology page linked from every chart.

## Operator tuning guide

Most users need none of this (defaults are good); this is for squeezing a busy node. `pulsate doctor` checks many of these and warns.

**OS / kernel (Linux):**
| Setting | Guidance |
|---|---|
| `ulimit -n` / `LimitNOFILE` | raise to ≥1M for high connection counts |
| `net.core.somaxconn` | raise (e.g., 65535) for high accept rates |
| `net.ipv4.ip_local_port_range` | widen for many upstream connections |
| `net.core.rmem_max`/`wmem_max`, UDP buffers | raise for HTTP/3/QUIC throughput |
| `net.ipv4.tcp_tw_reuse` | enable for high new-connection churn |
| transparent hugepages / THP | test; sometimes better off for latency |
| io_uring availability | enables the future thread-per-core backend ([10](10-performance.md)) |

**Pulsate config:**
| Lever | Effect |
|---|---|
| `pulsate { workers }` | prefork processes for SO_REUSEPORT scaling |
| `runtime { worker_threads, pin_workers }` | thread count & CPU affinity |
| `pool { max_idle, max_per_host }` | upstream connection reuse vs memory |
| `timeouts {}` | tighten to shed bad clients/backends faster |
| `limits {}` | bound memory under load |
| `tls { ciphers, min_version }` | `modern` for least CPU on capable clients |
| TLS session resumption | big win on handshake-heavy traffic |
| `compress { min_size, types }` | avoid compressing tiny/incompressible bodies |
| `cache` placement & store | offload origin; memory L1 + redis L2 for fleets |

**Topology:**
- Put Pulsate close to clients (edge) and use keep-alive to upstreams.
- For very high QUIC throughput, ensure GSO/GRO and adequate UDP buffers.
- On large NUMA boxes, pin workers and prefer local memory.
- Scale horizontally (stateless nodes) before vertically once a single box saturates the NIC.

## Cross-references
- [10. Performance](10-performance.md) — goals, lock-free design, zero-copy, kernel offloads.
- [28. Testing & Conformance](28-testing-and-conformance.md) — load/soak/chaos as release gates.
- [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — server-side measurement signals.
- [16. Deployment](16-deployment.md) — where these tunables apply in production.
- [13. CLI](13-cli.md) — `pulsate bench`, `pulsate doctor`.
