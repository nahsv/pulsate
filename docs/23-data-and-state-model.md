# 23. Data and State Model

> Everything Pulsate holds — in memory and on disk: the immutable config snapshot, the routing table, the certificate/ACME store, the cache index, cluster and rate-limit state, the on-disk layout, formats, encryption at rest, and how state survives restarts and reloads.

**Contents**
- [State taxonomy](#state-taxonomy)
- [In-memory state](#in-memory-state)
- [The ConfigSnapshot](#the-configsnapshot)
- [Persistent state & the storage engine](#persistent-state--the-storage-engine)
- [On-disk layout](#on-disk-layout)
- [Encryption at rest & permissions](#encryption-at-rest--permissions)
- [Shared (cluster) state](#shared-cluster-state)
- [Lifecycle: startup, reload, restart](#lifecycle-startup-reload-restart)
- [Cross-references](#cross-references)

---

## State taxonomy

| Class | Examples | Where | Survives restart |
|---|---|---|---|
| **Immutable config** | snapshot, routing table, handler configs | memory (`arc-swap`) | rebuilt from source |
| **Hot mutable** | rate-limit counters, breaker state, pool registries, in-flight | memory (sharded) | no (rebuilt) |
| **Cache** | stored responses + index | memory and/or disk/redis | disk/redis: yes |
| **Identity/secrets** | TLS certs/keys, ACME account, ticket keys | encrypted state store | yes |
| **Audit** | hash-chained change & security log | append-only store/sink | yes |
| **Cluster** | membership, leader lease, shared counters | gossip + redis/etcd | partially |

The design separates **derivable** state (rebuildable from config/source — never the source of truth) from **durable** state (certs, cache, audit) that must persist.

## In-memory state

Hot-path state is either immutable-shared or explicitly concurrent ([02. Architecture](02-architecture.md#memory-model)):
- **Immutable & `Arc`-shared:** the snapshot and everything reachable from it. Read lock-free.
- **Sharded concurrent maps** (striped/lock-free) for: rate-limit token buckets, circuit-breaker state per target, upstream connection-pool registries, the in-memory cache index, sticky-session tables. Sharded to avoid global contention; sized/bounded by config.
- **Per-task local:** connection and request buffers/state — share-nothing, freed when the task ends.
All in-memory mutable state is **bounded** so memory tracks configured limits, not input.

## The ConfigSnapshot

The central immutable object the data plane reads:

```
ConfigSnapshot {
  hash:            content hash (for diff/audit/version)
  generation:      monotonic counter
  routing_table:   { host index → route sets → (middleware list, handler) }
  upstreams_view:  name → Arc<UpstreamPool>     (pools live in a registry, referenced here)
  cert_view:       SNI → Arc<CertifiedKey>      (certs live in the cert store, referenced here)
  caches:          name → Arc<CacheHandle>
  plugins:         name → Arc<PluginInstancePool>
  globals:         timeouts, limits, log/metrics/tracing config
}
```

Key properties:
- **Built once, never mutated.** A reload builds a *new* snapshot; unchanged sub-objects (pools, certs, caches, plugin pools) are **referenced by `Arc` from the registries**, so reload reuses live resources rather than rebuilding them ([02. Architecture](02-architecture.md#hot-reload-architecture)).
- **Content-addressable:** the `hash` identifies a config version for diffing, audit, and rollback (the previous generation is retained for one step).

## Persistent state & the storage engine

- **Engine:** **redb** — a pure-Rust, embedded, transactional key-value store — keeps the one-binary, no-C-dependency promise (vs SQLite's C dependency or sled's stability concerns). ACID transactions protect cert/state writes.
- **Logical tables (key spaces):**
  - `certs` — cert + key + chain per identity, issuer, validity, source.
  - `acme` — account key, order/authorization state, challenge progress.
  - `tls_tickets` — rotating session-ticket keys.
  - `cache_index` — for the disk cache: key → blob location, validators, vary, tags, size, expiry (blobs themselves stored as files; see below).
  - `audit` — append-only, hash-chained records (or shipped to an external sink).
  - `meta` — instance ID, schema version, last-known-good snapshot hash.
- **Schema versioning & migrations:** the store carries a schema version; on upgrade, forward migrations run with a backup-first step ([32. Disaster Recovery & HA](32-disaster-recovery-and-ha.md)).

## On-disk layout

Default data dir `/var/lib/pulsate` (configurable; `pulsate { data_dir "..." }`):

```
/var/lib/pulsate/
├── state.redb                 # transactional KV: certs, acme, tickets, cache_index, meta
├── certs/                     # (optional) PEM mirrors for inspection (canonical copy in state.redb)
├── cache/
│   └── blobs/
│       ├── ab/abcd…           # content-addressed body blobs (sharded by hash prefix)
│       └── …
├── audit/
│   └── audit-2026-06-26.jsonl # hash-chained audit log (rotated), if file sink
├── plugins/                   # cached AOT-compiled plugin artifacts (+ source .wasm)
└── run/
    ├── pulsate.sock             # admin UDS (optional, alternative to TCP)
    └── pulsate.pid
```

- **Config (`pulsate.flow`)** is **not** here — it lives where the operator manages it (a repo, `/etc/pulsate`, a CRD). The data dir holds only runtime/durable state.
- **Cache blobs** are content-addressed files (enabling dedup and `sendfile`); the index in `state.redb` maps cache keys → blobs. Disk cache has its own size cap with LRU/TinyLFU eviction and periodic compaction.
- **Audit** can be a local rotated JSONL file or streamed to syslog/OTLP (then the local file is just a buffer).

## Encryption at rest & permissions

- **Sensitive material encrypted at rest:** private keys, ACME account keys, ticket keys, and secret-derived values in `state.redb` are encrypted with a data-encryption key wrapped by the secrets backend/KMS ([09. Security](09-security.md)). Without the KMS/key, a stolen disk yields no keys.
- **File permissions:** the data dir is `0700`, owned by the `pulsate` user; key material `0600`. Pulsate refuses to start (or warns loudly) on world-readable state.
- **Secrets never persisted in plaintext** and never written to logs/audit/cache (redaction is enforced).
- **Cache confidentiality:** responses marked `private`/authenticated are not written to a shared on-disk/redis cache unless explicitly keyed by identity; optional cache encryption for sensitive deployments.

## Shared (cluster) state

In cluster mode ([16. Deployment](16-deployment.md)), some state is externalized so nodes act as one:
- **Certificates & ACME:** stored in a shared backend (or replicated) so issuance happens once (leader) and all nodes load the same certs.
- **Cache L2:** Redis-backed shared cache; per-node `state.redb`/memory is L1.
- **Distributed rate-limit counters & sticky tables:** Redis/cluster-backed for fleet-consistent enforcement (with local fast-path approximations).
- **Membership & leader lease:** gossip (base) or etcd/consul (strong consistency option).
- **Consistency:** certs/config favor strong consistency (correctness); cache/counters favor availability (eventual, bounded staleness). These choices are documented per datum so operators understand the trade-offs.

## Lifecycle: startup, reload, restart

```
startup ─▶ open state.redb (migrate if needed) ─▶ load certs/acme/tickets
        ─▶ load config source → build snapshot ─▶ rehydrate disk cache index
        ─▶ bind sockets ─▶ publish snapshot ─▶ ready
reload  ─▶ build new snapshot (reuse pools/certs/caches via registries) ─▶ atomic swap
restart ─▶ derivable state rebuilt; durable state (certs/cache/audit) reloaded from disk
crash   ─▶ redb transactions ensure no torn cert/state writes; cache index self-heals (orphan blobs GC'd)
```

- **No data loss on graceful shutdown:** cache index and state are flushed; in-flight audit records committed.
- **Crash safety:** transactional writes mean a crash never leaves a half-written cert or corrupt index; a startup integrity check GCs orphaned cache blobs and verifies the audit hash chain.
- **Backups** operate on the data dir + a config snapshot export ([32. Disaster Recovery & HA](32-disaster-recovery-and-ha.md)).

## Cross-references
- [02. Architecture](02-architecture.md) — snapshot model, registries, memory model, reload.
- [08. Cache](08-cache.md) — cache index/blob semantics and eviction.
- [09. Security](09-security.md) — encryption at rest, secret handling, audit.
- [16. Deployment](16-deployment.md) & [32. DR/HA](32-disaster-recovery-and-ha.md) — shared state, backup/restore.
- [33. Release Engineering](33-release-engineering-and-supply-chain.md) — schema migration & upgrade safety.
