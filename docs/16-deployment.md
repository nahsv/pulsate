# 16. Deployment

> Running Pulsate everywhere it needs to run: bare metal, Docker, Compose, Kubernetes, systemd, and the cloud — single-node and as a coordinated multi-node cluster.

**Contents**
- [Deployment principles](#deployment-principles)
- [Bare metal](#bare-metal)
- [systemd](#systemd)
- [Docker](#docker)
- [Docker Compose](#docker-compose)
- [Kubernetes](#kubernetes)
- [Cloud](#cloud)
- [Multi-node & cluster mode](#multi-node--cluster-mode)
- [Zero-downtime upgrades](#zero-downtime-upgrades)
- [Cross-references](#cross-references)

---

## Deployment principles

- **The same binary and the same config everywhere.** A laptop, a VM, a pod, and a 50-node fleet run the identical `p8` and shape of `pulsate.flow`. No special "ingress build."
- **Stateless data plane.** Each node serves from an immutable snapshot; shared state (certs, cache L2, rate-limit counters, sticky sessions) is externalized so nodes are interchangeable and horizontally scalable.
- **Self-sufficient single node, optional coordination.** One Pulsate is complete. Cluster mode adds coordination (shared certs/state, fleet config) without requiring an external control plane.
- **Secure & observable by default in every target.** TLS, loopback admin, metrics, and audit logging behave the same regardless of platform.

## Bare metal

- Drop the static binary on the host, write `/etc/p8/pulsate.flow`, run `p8 run` (foreground) under a supervisor, or `p8 up --detach`.
- **Privileged ports:** bind 80/443 via `CAP_NET_BIND_SERVICE` (preferred) or start as root and **drop to an unprivileged user** after binding (`pulsate { user "p8"; group "p8" }`).
- **Tuning:** raise file-descriptor limits, set `somaxconn`, ephemeral port range, and (for high QUIC throughput) UDP buffer sysctls — `p8 doctor` checks these and [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md) documents recommended values.
- State (certs, cache, audit) lives under a configurable data dir (`/var/lib/p8`), which should be on persistent storage ([23. Data & State Model](23-data-and-state-model.md)).

## systemd

A first-class unit ships with the packages:

```ini
[Unit]
Description=Pulsate Application Gateway
After=network-online.target
Wants=network-online.target

[Service]
Type=notify                       # Pulsate signals readiness via sd_notify
ExecStart=/usr/bin/p8 run --config /etc/p8/pulsate.flow
ExecReload=/usr/bin/p8 reload  # zero-downtime reload on `systemctl reload p8`
AmbientCapabilities=CAP_NET_BIND_SERVICE
User=p8
Group=p8
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/p8
LimitNOFILE=1048576
Restart=on-failure
WatchdogSec=30s                   # systemd watchdog integration

[Install]
WantedBy=multi-user.target
```

- `Type=notify` + `sd_notify` means systemd knows when Pulsate is actually ready (after binding + first snapshot), gating dependents correctly.
- `systemctl reload` maps to the zero-downtime config reload; the hardening directives (`ProtectSystem`, `NoNewPrivileges`) reflect secure-by-default.

## Docker

- **Distroless, multi-arch images**; tiny (one static binary). Run as non-root by default; `CAP_NET_BIND_SERVICE` granted for low ports.
  ```bash
  docker run -d --name p8 \
    -p 80:80 -p 443:443 -p 443:443/udp \
    -v $PWD/pulsate.flow:/etc/p8/pulsate.flow:ro \
    -v pulsate-data:/var/lib/p8 \
    ghcr.io/p8/p8:1
  ```
- **HTTP/3** needs the UDP port published (`443/udp`).
- **Config & secrets** mount read-only; data volume persists certs/cache/state. Health via `HEALTHCHECK` hitting the readiness endpoint.

## Docker Compose

Pulsate as the edge for a Compose stack, with service auto-discovery:

```yaml
services:
  p8:
    image: ghcr.io/p8/p8:1
    ports: ["80:80", "443:443", "443:443/udp"]
    volumes:
      - ./pulsate.flow:/etc/p8/pulsate.flow:ro
      - pulsate-data:/var/lib/p8
    depends_on: [api, web]
  api:
    build: ./api          # reachable as upstream http://api:8080
  web:
    build: ./web
volumes: { pulsate-data: {} }
```

```
# pulsate.flow
site app.example.com {
  tls auto
  route /api/* ~> strip_prefix("/api") ~> proxy(http://api:8080)
  route /*     ~> proxy(http://web:3000)
}
```

Compose service names resolve as upstreams; optional **label-based** config lets containers declare their own routes if you prefer that style (file stays canonical).

## Kubernetes

Pulsate runs as a Deployment/DaemonSet and acts as ingress/gateway (see [14. DX — Kubernetes](14-developer-experience.md#kubernetes-support)):

- **Install:** Helm chart or an operator; CRDs for `Gateway API` (`Gateway`/`HTTPRoute`/`GRPCRoute`) and the native `PulsateConfig`/`PulsateRoute`.
- **Topology:** typically a Deployment behind a `Service type=LoadBalancer` (cloud LB) or a DaemonSet with `hostNetwork`/NodePort at the edge.
- **Config source:** the control plane watches CRDs/Gateway resources and builds snapshots — config lives in Kubernetes, reconciled continuously, GitOps-friendly.
- **Discovery:** native EndpointSlice watching feeds dynamic upstreams ([06. Reverse Proxy](06-reverse-proxy.md)); pod churn needs no reload.
- **Shared state across replicas:** ACME cert issuance is **leader-elected** (issue once, share to all pods via the cluster/state store), and cache L2 + rate-limit counters use Redis/cluster so replicas behave as one logical gateway ([Multi-node](#multi-node--cluster-mode)).
- **Ops:** Prometheus `ServiceMonitor`, OTLP traces, liveness/readiness/startup probes, PodDisruptionBudget for safe rollouts, HPA on rps/CPU.

## Cloud

- **Managed LBs:** sits behind AWS NLB/ALB, GCP LB, Azure LB; supports PROXY protocol (v1/v2) to recover the real client IP when behind an L4 LB.
- **Cloud-native integrations:** secrets from AWS/GCP/Azure managers and KMS for at-rest encryption ([09. Security](09-security.md)); cloud DNS providers for ACME DNS-01 and wildcards.
- **Autoscaling:** stateless nodes scale on rps/CPU; shared state lives in managed Redis + the cluster store.
- **Images** published to major registries; Terraform/Pulumi modules and reference architectures provided ([17. Documentation](17-documentation.md)).

## Multi-node & cluster mode

`pulsate-cluster` lets independent nodes act as one logical gateway:

```
cluster {
  id "edge-eu"
  peers ["10.0.0.1:7700", "10.0.0.2:7700"]   # or discover dns/kubernetes
  bind 0.0.0.0:7700
  state { certs shared; cache redis://...; rate_limit shared; sticky shared }
}
```

- **Membership:** a lightweight gossip protocol tracks node liveness; no external coordinator required for the base case (an optional etcd/consul backend is available for strong consistency where wanted).
- **Shared cert issuance:** one node (leader) solves ACME challenges and stores certs in the shared state; all nodes load them — avoids per-node issuance and rate-limit problems.
- **Shared cache & limits:** Redis-backed L2 cache and distributed rate-limit counters give consistent behavior across nodes; per-node L1 keeps it fast ([08. Cache](08-cache.md), [09. Security](09-security.md)).
- **Config distribution:** in cluster mode, an applied config can propagate to peers (each validates and atomically swaps its own snapshot), or each node reads the same GitOps/CRD source. Roll-forward and rollback are coordinated.
- **No single point of failure on the data path:** any node can serve any request; losing a node degrades capacity, not correctness. HA/DR specifics in [32. Disaster Recovery & HA](32-disaster-recovery-and-ha.md).

## Zero-downtime upgrades

- **Config:** always zero-downtime via snapshot swap.
- **Binary, single node:** `p8 upgrade --zero-downtime` performs **socket handoff** — a new process inherits the listening sockets (SCM_RIGHTS/`SO_REUSEPORT`) and starts serving while the old process drains in-flight requests, then exits. No dropped connections.
- **Binary, fleet:** rolling upgrade — drain one node (readiness off → LB stops sending traffic → drain → upgrade → readiness on), repeat. PodDisruptionBudget/`maxUnavailable` controls pace on Kubernetes.
- **Rollback:** keep the previous binary and snapshot; a failed upgrade rolls back the node and the config generation.

## Cross-references
- [02. Architecture](02-architecture.md) — snapshot model, graceful shutdown, worker lifecycle, socket handoff.
- [14. Developer Experience](14-developer-experience.md) — Docker/Compose/K8s from the developer side.
- [32. Disaster Recovery & HA](32-disaster-recovery-and-ha.md) — failover, backup/restore, RPO/RTO.
- [23. Data & State Model](23-data-and-state-model.md) — the data dir and shared state stores.
- [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md) — OS/kernel tuning for production.
