# 01. Vision

> The product thesis behind Pulsate: mission, philosophy, who it is for, what it deliberately is not, how it differs from the incumbents, and where it goes over a five-year horizon.

**Contents**
- [Mission](#mission)
- [Design philosophy](#design-philosophy)
- [Target users](#target-users)
- [Non-goals](#non-goals)
- [Comparison with existing reverse proxies](#comparison-with-existing-reverse-proxies)
- [Long-term roadmap (5+ years)](#long-term-roadmap-5-years)
- [Cross-references](#cross-references)

---

## Mission

**Pulsate is the application gateway a single developer can run in thirty seconds and a platform team can run a global fleet of — without changing tools in between.**

The reverse-proxy market is split into two painful halves. On one side sit *developer-friendly* tools (Caddy) that are wonderful for a single box but thin on the controls a platform team needs. On the other sit *infrastructure-grade* tools (Envoy, Nginx-as-ingress) that are enormously capable but require a control plane, a sidecar mesh, a templating layer, or a team of specialists to operate. Developers who start on the easy tool hit a wall and rewrite; teams who start on the hard tool pay a permanent operability tax.

Pulsate refuses that trade-off. The bet is that **secure-by-default ergonomics and production-grade capability are not in tension** — they were only ever separated because no one rebuilt the stack from first principles in a memory-safe language with a single, coherent configuration model. Pulsate is that rebuild: one statically-linked binary, one human-writable config file, one command to run it, and a batteries-included feature set (automatic TLS, caching, WAF, auth, rate limiting, observability, a dashboard, and a WASM plugin system) that you grow *into* rather than *out of*.

Concretely, success means:
- A newcomer serves a real app over HTTPS, with a valid certificate, in under a minute, having read nothing.
- That same config file scales — unchanged in shape — to a multi-node, multi-tenant, enterprise deployment.
- Operators never reach for a second tool to get caching, security, or metrics.
- Extending Pulsate never requires forking it.

## Design philosophy

Ten pillars. Each is a *constraint* the architecture is held to, not a slogan.

1. **One binary.** Pulsate ships as a single static executable with no runtime dependencies — no OpenSSL, no Lua interpreter, no Node process for the UI, no separate agent. Rationale: distribution and operability are features. Every external dependency is a deployment failure mode and a supply-chain surface. (Enforced by technology choices: rustls over OpenSSL, an embedded Svelte dashboard, an in-process WASM runtime — see [Canon / Technology decisions](02-architecture.md).)
2. **One config.** A single file, `pulsate.flow`, written in an original, purpose-built configuration language (see [04. Configuration](04-configuration.md)). Not YAML, not a templated `nginx.conf`, not an annotation soup spread across Kubernetes objects. Configuration is the primary product surface and is designed as such.
3. **One command.** `p8 up` takes you from nothing to a running, TLS-terminated gateway. Everything else (`validate`, `reload`, `cert`, `bench`, `import`) is a subcommand of the same binary (see [13. CLI](13-cli.md)).
4. **Secure by default.** TLS is automatic. Sane security headers are on. The admin API is loopback-only until you say otherwise. Defaults assume hostile input. You opt *out* of safety, never *in*. (See [09. Security](09-security.md) and [21. Threat Model](21-threat-model.md).)
5. **Batteries included.** Caching, WAF, rate limiting, compression, CORS, JWT/mTLS auth, health checks, load balancing, metrics, tracing, and a dashboard are core features, not plugins to assemble. Rationale: the 95% case should require zero ecosystem archaeology.
6. **Extremely easy to use.** Error messages point at the line and column and suggest the fix. The CLI explains what it is about to do. The dashboard makes the running state legible. Ease is measured, not asserted (time-to-first-byte for a new user is a tracked metric).
7. **Production-ready.** Graceful reloads with zero dropped connections, bounded memory, backpressure everywhere, structured audit logs, and first-class observability from day one. (See [10. Performance](10-performance.md), [15. Observability](15-observability.md).)
8. **Extensible.** A sandboxed WebAssembly plugin system with a stable, versioned host interface lets anyone add middleware, handlers, matchers, and integrations without forking or recompiling Pulsate (see [12. Plugins](12-plugins.md)).
9. **Cloud-native.** Pulsate speaks Kubernetes (Gateway API + a native CRD), discovers services dynamically, exposes Prometheus/OpenTelemetry, and runs identically on a laptop, a VM, and a pod (see [14. Developer Experience](14-developer-experience.md), [16. Deployment](16-deployment.md)).
10. **Enterprise-capable.** Multi-tenancy, RBAC, audit logging, secrets backends, clustering, and a path to a commercially-supported edition (see [29. Multi-Tenancy](29-multi-tenancy-and-isolation.md), [20. Future](20-future.md)).

A useful way to hold these together: **Pulsate optimizes for the *time-to-correct-configuration*, not just time-to-running.** Many tools get you running fast and correct slowly. Pulsate treats "running, secure, observable, and correct" as the same milestone.

## Target users

Pulsate is designed for three concentric audiences, in priority order.

| Audience | Who they are | What they need from Pulsate | Primary docs |
|---|---|---|---|
| **The solo developer / indie team** | Ships a Rails/Node/Go/Rust app; wants HTTPS, a domain, maybe caching; has no ops team | `p8 up` and a five-line config; automatic certs; app auto-detection | [14. DX](14-developer-experience.md), [04. Config](04-configuration.md) |
| **The platform / infra team** | Runs many services for many internal teams; cares about reload safety, blast radius, observability, policy | Multi-site config, health checks, circuit breakers, metrics, audit logs, clustering | [06. Reverse Proxy](06-reverse-proxy.md), [02. Architecture](02-architecture.md) |
| **The enterprise** | Compliance, multi-tenancy, support contracts, air-gapped installs | RBAC, secrets backends, FIPS posture, HA/DR, commercial support | [29. Multi-Tenancy](29-multi-tenancy-and-isolation.md), [32. DR/HA](32-disaster-recovery-and-ha.md) |

The crucial design discipline: **we serve the platform team and enterprise without taxing the solo developer.** Advanced capability is *present but quiet* — defaults are simple, complexity is revealed progressively as the config grows.

Secondary audiences explicitly considered: plugin authors (need a stable SDK — [12. Plugins](12-plugins.md)), and tool integrators embedding Pulsate as a library (the crate split in [03. Repository](03-repository.md) keeps the data plane usable as a dependency).

## Non-goals

Stating what Pulsate will *not* be is as important as what it will. These are deliberate, not temporary.

- **Not a forward/egress proxy or a general SOCKS proxy.** Pulsate is an *ingress* application gateway. Outbound proxying is out of scope.
- **Not a full service mesh.** Pulsate can be the edge and the per-node gateway, and clusters can coordinate, but Pulsate does not ship a per-pod sidecar mesh with mTLS-everywhere identity fabric in v1. (A mesh-adjacent mode is a *future* consideration — [20. Future](20-future.md) — not a launch promise.)
- **Not a programmable L4 load balancer first.** Pulsate does L4 TCP/UDP forwarding as a supporting feature, but its center of gravity is L7 HTTP(S). It is not trying to replace a kernel-level LB like IPVS.
- **Not a config *templating* engine.** Pulsate will not adopt Go templates / Jinja over its config. Dynamic behavior comes from typed config, the admin API, and plugins — not string interpolation of the config language.
- **Not a YAML tool.** The configuration language is purpose-built. We will provide a YAML/JSON *interop* import for machine integration, but the human surface is `.flow`.
- **Not tied to one orchestrator.** Kubernetes is first-class but not required. Pulsate must be excellent on bare metal and systemd.
- **Not a research playground for novel protocols.** Pulsate implements the standards (HTTP/1.1, /2, /3, TLS 1.2/1.3, QUIC, gRPC, WebSocket, SSE) correctly and conservatively rather than inventing wire formats.

## Comparison with existing reverse proxies

We learn from all four incumbents and copy none of them. This table is an honest positioning, not marketing.

| Dimension | **Pulsate** | Caddy | Nginx | Envoy | Traefik |
|---|---|---|---|---|---|
| Language / safety | Rust (memory-safe) | Go (GC) | C (manual memory) | C++ (manual memory) | Go (GC) |
| Distribution | One static binary, no deps | One binary | Binary + modules, OpenSSL | Binary, large | One binary |
| Automatic TLS | ✅ default | ✅ default | ❌ (manual/certbot) | ⚠️ via control plane | ✅ |
| Config surface | Purpose-built `.flow` DSL | Caddyfile / JSON | `nginx.conf` directives | YAML/xDS (control plane) | YAML/labels/CRD |
| Caching built in | ✅ (mem/disk/redis, RFC-correct) | ⚠️ plugin | ✅ (proxy_cache) | ⚠️ filter | ❌ |
| WAF built in | ✅ core | ⚠️ plugin | ⚠️ ModSecurity addon | ⚠️ ext_authz/filters | ❌ |
| Extensibility | WASM (sandboxed, stable ABI) | Go modules (recompile) | C modules / Lua | C++ filters / WASM | Go middleware/plugins |
| Dashboard | ✅ embedded | ❌ | ❌ (commercial Plus) | ⚠️ (separate) | ✅ |
| Control/data plane split | ✅ in-process, snapshot-based | partial | reload-fork | external (xDS) | internal |
| HTTP/3 | ✅ core (quinn) | ✅ | ⚠️ recent | ✅ | ✅ |
| Primary pain it removes | "I need a second tool" / "I outgrew it" | outgrowing it; few enterprise controls | manual TLS; imperative config; C CVEs | operational complexity; needs control plane | YAML/label sprawl; performance ceiling |

**The synthesis Pulsate is making:** Caddy's *ergonomics and auto-TLS*, Nginx's *raw efficiency mindset*, Envoy's *control/data-plane discipline and observability*, and Traefik's *dynamic, cloud-native discovery* — unified in one memory-safe binary with an original config model, so that no single one of those strengths costs you another's. Crucially, the control/data-plane split that makes Envoy powerful is brought *in-process* via an atomically-swapped configuration snapshot (see [02. Architecture](02-architecture.md)), giving the safety without the external control plane.

## Long-term roadmap (5+ years)

A directional roadmap. Exact dates live in [19. Milestones](19-milestones.md); this is the multi-year arc.

**Year 1 — Foundations & "it just works."**
Data plane (HTTP/1.1, /2, TLS, routing, reverse proxy, middleware), the Flow config language, automatic ACME, the CLI, core observability, and the first dashboard. Target: a developer's favorite single-node gateway. Public 0.x, then 1.0 with a stable config format and HTTP semantics.

**Year 2 — Platform & extensibility.**
HTTP/3 + QUIC GA, the WASM plugin system and SDK with a stable host ABI, full caching subsystem, WAF maturity, Kubernetes integration (Gateway API + native CRD), clustering for shared state, and dynamic service discovery. Target: the platform team's default.

**Year 3 — Ecosystem & scale.**
Plugin marketplace, advanced traffic management (progressive delivery, traffic shadowing, fault injection), multi-tenancy with RBAC, secrets-backend integrations, and hardened HA/DR. Performance work: thread-per-core io_uring data-plane backend behind `pulsate-rt`. Target: credible enterprise deployments.

**Year 4 — Enterprise & cloud control plane.**
A managed/self-hosted control plane for fleet-wide config, certificate, and policy management across regions; an enterprise edition with support, compliance attestations (SOC 2, optional FIPS-validated crypto build), and air-gapped operation. Target: regulated and large-fleet customers.

**Year 5+ — Intelligent edge.**
AI-assisted diagnostics ("why is this route slow / returning 502?"), anomaly-based WAF tuning, capacity recommendations, and multi-region active-active edge with global cache coherence. Pulsate becomes not just a gateway but an *operator co-pilot* for the edge.

Throughout, two invariants hold: **the single-binary/single-config experience never regresses**, and **the open-source core stays genuinely capable** — enterprise features are additive, never a crippling of the OSS edition (see [18. Open Source](18-open-source.md), [20. Future](20-future.md)).

## Cross-references
- [02. Architecture](02-architecture.md) — how the philosophy becomes a system.
- [04. Configuration](04-configuration.md) — the Flow language that realizes "one config."
- [09. Security](09-security.md) & [21. Threat Model](21-threat-model.md) — "secure by default" in detail.
- [18. Open Source](18-open-source.md) & [20. Future](20-future.md) — governance and the commercial arc.
- [19. Milestones](19-milestones.md) — the dated execution plan behind the roadmap.
