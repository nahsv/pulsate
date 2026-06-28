# 32. Disaster Recovery and High Availability

> Keeping Pulsate available through failures and recoverable from disasters: redundancy and failover models, backup and restore of certs/config/state, ACME key recovery, replication and consistency, RPO/RTO targets, rolling upgrades and rollback, split-brain handling, and operational runbooks.

**Contents**
- [Availability model](#availability-model)
- [Single-node resilience](#single-node-resilience)
- [Multi-node high availability](#multi-node-high-availability)
- [Backup & restore](#backup--restore)
- [Certificate & ACME recovery](#certificate--acme-recovery)
- [Replication & consistency](#replication--consistency)
- [RPO / RTO targets](#rpo--rto-targets)
- [Upgrades & rollback](#upgrades--rollback)
- [Split-brain & failure handling](#split-brain--failure-handling)
- [Runbooks](#runbooks)
- [Cross-references](#cross-references)

---

## Availability model

Pulsate is designed so that **the data path has no mandatory single point of failure** and **no external dependency is required to keep serving** existing config. The control plane can be degraded (can't issue new certs, can't reload) while the data plane keeps serving from its last good snapshot — availability degrades gracefully from "fully managed" to "still serving traffic." This separation ([02. Architecture](02-architecture.md)) is the backbone of the HA story.

## Single-node resilience

Even one node is resilient within its limits:
- **Supervised workers:** a crashed worker/listener is restarted with backoff; a panic in one request is isolated and never takes down the process ([02. Architecture](02-architecture.md#error-handling)).
- **Last-good snapshot:** a rejected reload keeps the running snapshot; a process restart reloads durable state (certs/cache) from disk and rebuilds the snapshot.
- **Crash-safe state:** transactional `state.redb` means no torn cert/state writes; startup integrity-checks and self-heals (orphan cache GC, audit hash-chain verify) ([23. Data & State Model](23-data-and-state-model.md)).
- **Serve-stale shield:** `stale-if-error` + circuit breakers let a node keep answering from cache when origins fail ([08. Cache](08-cache.md), [06. Reverse Proxy](06-reverse-proxy.md)).
A single node is still a SPOF for *availability* (the box can die) — production HA needs multiple nodes.

## Multi-node high availability

`pulsate-cluster` ([16. Deployment](16-deployment.md)) gives true HA:
- **N stateless nodes** behind a load balancer / anycast; any node serves any request. Losing a node reduces capacity, not correctness.
- **Health-gated traffic:** readiness drives the LB; a draining/unhealthy node stops receiving traffic before it stops serving.
- **Shared identity/state:** certs, cache L2, rate-limit budgets, and sticky tables live in shared backends so nodes are interchangeable.
- **No control-plane SPOF on the data path:** the cluster coordinates config/cert issuance, but a node serves traffic even if it temporarily can't reach peers.
- **Zone/region spread:** nodes spread across availability zones; multi-region active-active is the [20. Future](20-future.md) extension.

## Backup & restore

What to back up and how:
- **Config (`pulsate.flow`):** the source of truth lives in your VCS/CRD — that *is* the config backup (GitOps). `p8 config dump --effective` captures the resolved view for audit.
- **State (`data_dir`):** certs, ACME account/keys, ticket keys, cache index, audit. Back up via `p8 backup` (a consistent snapshot of `state.redb` + manifests) on a schedule to off-host/object storage. Cache *blobs* are regenerable and can be excluded to shrink backups.
- **Restore:** `p8 restore <backup>` repopulates `data_dir`; on start, Pulsate validates schema, decrypts keys via the KMS/secrets backend, and resumes. Certs are immediately usable (no re-issuance needed).
- **Encryption:** backups are encrypted (KMS-wrapped) so a leaked backup doesn't leak keys ([09. Security](09-security.md)).
- **Tested restores:** restore drills are part of the runbooks (a backup you haven't restored is a hope, not a backup).

## Certificate & ACME recovery

Certificates are the highest-stakes state (losing the ACME account key means re-registration; rate limits make rapid re-issuance painful):
- **Account key is backed up** and, in a cluster, **shared** so any node/region can renew.
- **Certs survive node loss** (shared store / backup), so a replaced node serves immediately without solving challenges.
- **Renewal redundancy:** leader-elected renewal with failover — if the leader dies, a new leader takes over renewal; near-expiry alerts fire well ahead ([26. Metrics Catalog](26-metrics-and-slo-catalog.md)).
- **Disaster path:** if certs are lost entirely, on-demand/bulk re-issuance is possible but rate-limited — hence the emphasis on backing up the account key and certs.

## Replication & consistency

Different data, different consistency (documented per datum — [23. Data & State Model](23-data-and-state-model.md)):
| Data | Consistency | Rationale |
|---|---|---|
| Certs / ACME state | strong | correctness; can't serve two truths |
| Config generation | strong (coordinated apply) | all nodes converge to one version |
| Cache L2 | eventual (bounded) | availability > perfect freshness; TTL/SWR tolerate it |
| Rate-limit counters | eventual + local fast-path | approximate is fine; exactness isn't worth a network hop per request |
| Sticky tables | eventual | re-pin on miss is acceptable |
Strong-consistency needs use the configured backend (leader lease / etcd-consul option); availability-favoring data uses Redis/gossip.

## RPO / RTO targets

Recommended targets (operators set per SLA):
| Scenario | RPO (data loss) | RTO (recovery time) |
|---|---|---|
| Single node crash (HA cluster) | 0 (peers serve) | seconds (LB reroutes) |
| Single node, no cluster | ≤ backup interval for state; 0 for config (in VCS) | minutes (restart/restore) |
| Region loss (multi-region) | ~0 for replicated certs/config; bounded for cache | seconds–minutes (failover) |
| Full data-dir loss | depends on last backup (target ≤ 1h) | minutes (restore) + cert re-issue if account key lost |
Config RPO is effectively zero because config lives in version control.

## Upgrades & rollback

- **Config:** zero-downtime snapshot swap with one-generation auto-rollback on elevated errors ([02. Architecture](02-architecture.md#hot-reload-architecture)).
- **Binary, single node:** socket-handoff zero-downtime upgrade; keep the prior binary for instant rollback.
- **Binary, fleet:** rolling upgrade gated by readiness + PodDisruptionBudget/`maxUnavailable`; canary one node, watch SLOs, proceed or roll back ([16. Deployment](16-deployment.md)).
- **State schema:** migrations are backup-first and reversible within a version window.

## Split-brain & failure handling

- **Leader election** (for cert issuance / coordinated apply) uses a lease; if the cluster partitions, only the partition holding the lease quorum acts as leader — the other side keeps **serving** but does not issue certs / apply global changes, avoiding divergent truth.
- **Data-plane is partition-tolerant:** serving from local snapshot + cache continues on both sides of a partition; only coordinated *writes* are gated.
- **Reconciliation on heal:** when the partition heals, config generations reconcile to the highest committed version; cache/counter divergence is reconciled by TTL/eventual convergence; audit logs merge (hash-chained per node, then ordered).
- **Fail-safe defaults:** when uncertain (e.g., can't reach the rate-limit backend), the node uses a local conservative policy rather than failing requests, configurable as fail-open/closed per concern.

## Runbooks

Shipped operational runbooks (in [17. Documentation](17-documentation.md)) cover, step by step:
- Node failure & replacement; draining for maintenance.
- Backup creation, verification, and full restore drill.
- Certificate emergency (renewal failure / near-expiry / account-key loss).
- Reload gone wrong (rollback) and binary upgrade rollback.
- Cluster partition response and post-heal verification.
- Region failover and failback.
- Cache/origin incident (enable serve-stale, purge after fix).
Each runbook lists the exact `p8`/admin-API commands, the metrics/alerts that trigger it, and verification steps.

## Cross-references
- [16. Deployment](16-deployment.md) — cluster topology and zero-downtime upgrades.
- [23. Data & State Model](23-data-and-state-model.md) — what persists and how it's protected.
- [09. Security](09-security.md) — encrypted backups, key management.
- [02. Architecture](02-architecture.md) — snapshot/reload/supervision underpinning resilience.
- [20. Future](20-future.md) — multi-region active-active.
