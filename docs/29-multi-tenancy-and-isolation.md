# 29. Multi-Tenancy and Isolation

> Running many tenants on shared Pulsate infrastructure safely: the tenant model, namespacing, per-tenant resource limits and quotas, noisy-neighbor protection, RBAC-scoped config, isolated telemetry and audit, and per-tenant plugin sandboxing — the foundation for enterprise and the managed service.

**Contents**
- [When multi-tenancy applies](#when-multi-tenancy-applies)
- [Tenant model](#tenant-model)
- [Namespacing & config scoping](#namespacing--config-scoping)
- [Resource limits & quotas](#resource-limits--quotas)
- [Noisy-neighbor protection](#noisy-neighbor-protection)
- [RBAC & control-plane isolation](#rbac--control-plane-isolation)
- [Telemetry, logs & audit isolation](#telemetry-logs--audit-isolation)
- [Plugin & data isolation](#plugin--data-isolation)
- [Blast-radius containment](#blast-radius-containment)
- [Cross-references](#cross-references)

---

## When multi-tenancy applies

Three scenarios need tenant isolation: an internal platform team serving many product teams from one gateway fleet; a SaaS routing many customers' domains through shared Pulsate nodes; and Pulsate Cloud, the managed service ([20. Future](20-future.md)). The single-tenant developer never pays for this — multi-tenancy is opt-in and layered on the same engine. The relevant threats (cross-tenant access, noisy neighbors, shared-cache collisions) are catalogued in [21. Threat Model](21-threat-model.md).

## Tenant model

A **tenant** is a named isolation boundary owning a set of sites/routes, certs, quotas, RBAC subjects, telemetry scope, and plugin grants:

```
tenant acme {
  sites   ["*.acme.example.com", "shop.acme.com"]
  quota   { rps 5000; connections 50000; bandwidth 1Gbps; cache 2GB; certs 50 }
  rbac    { admins ["oidc:group/acme-admins"]; viewers ["oidc:group/acme-all"] }
  plugins { allow [geoblock]; capabilities { net ["api.acme.internal:443"] } }
  isolation strict        # strict | shared (cache/limits sharing posture)
}
```

Tenants are typically expressed via `use "tenants/*.flow"` includes ([04. Configuration](04-configuration.md)) — each tenant ships its own file, validated and namespaced. A tenant cannot define sites/hosts outside its `sites` allow-list (enforced at validation: `PLS-CFG-*`).

## Namespacing & config scoping

- **Host ownership:** a tenant may only claim hostnames within its allow-list; collisions across tenants are validation errors (no two tenants serve the same host). This prevents hostname hijacking.
- **Reference scoping:** `@upstream`/`@cache`/`@waf` references resolve **within the tenant** by default; sharing a definition across tenants is explicit. A tenant cannot reference another tenant's resources.
- **Cache key namespacing:** every cache entry is implicitly prefixed with the tenant ID, so two tenants caching `/index.html` never collide or leak — even on a shared store ([08. Cache](08-cache.md)).
- **Rate-limit/sticky key namespacing:** counters and affinity tables are tenant-prefixed.

## Resource limits & quotas

Per-tenant quotas turn shared capacity into fair, bounded slices:

| Quota | Enforced by |
|---|---|
| `rps` / request rate | tenant-scoped rate limiter (distributed in cluster) |
| `connections` | per-tenant connection accounting at the listener |
| `bandwidth` | per-tenant egress/ingress shaping |
| `cache` (bytes) | per-tenant cache budget with independent eviction |
| `certs` | cap on issued certificates (ACME-abuse guard) |
| `cpu` (plugin fuel) | per-tenant aggregate plugin fuel budget |

Exceeding a quota yields a tenant-scoped `429`/`503` (`PLS-SEC-0004`/`PLS-PRX-0005`) — it never spills into other tenants' budgets. Quotas map cleanly to pricing tiers in the managed/enterprise editions.

## Noisy-neighbor protection

Isolation is meaningless if one tenant can starve others:
- **Fair scheduling:** request admission and worker time are accounted per tenant; a tenant at its quota is throttled, not allowed to consume the shared pool.
- **Independent connection/memory budgets:** a tenant flooding connections hits its own `connections` cap first.
- **Per-tenant circuit isolation:** one tenant's failing upstream (retries, breaker churn) is contained to that tenant's pools and budgets ([06. Reverse Proxy](06-reverse-proxy.md)).
- **Cache isolation:** a tenant churning the cache evicts only within its own budget, not others' hot objects.
- **Plugin CPU isolation:** per-tenant fuel budgets stop a tenant's plugin from monopolizing CPU ([12. Plugins](12-plugins.md)).

## RBAC & control-plane isolation

- **Scoped admin access:** RBAC subjects are tenant-scoped — a tenant admin can edit only their tenant's config, certs, and cache via the [Admin API](22-admin-api.md)/dashboard; they cannot see or touch other tenants or global engine settings.
- **Roles:** platform-admin (global), tenant-admin (their tenant), tenant-operator (purge/renew within tenant), tenant-viewer. Backed by SSO groups in the enterprise edition.
- **Self-service, bounded:** tenants can manage their own routes/certs within their quota and host allow-list — reducing platform-team toil — without any cross-tenant capability.
- **Config validation enforces boundaries:** a tenant's config that references out-of-scope hosts/resources fails validation before it can ever apply.

## Telemetry, logs & audit isolation

- **Metrics** carry a `tenant` label (within the cardinality budget — [26. Metrics Catalog](26-metrics-and-slo-catalog.md)), so each tenant gets its own dashboards/SLOs and a platform view aggregates them.
- **Logs/traces** are tenant-tagged; a tenant's view (dashboard/API) shows only their requests; the platform team sees all.
- **Audit** is tenant-scoped and separately queryable — a tenant sees their own change history; security/compliance needs per-tenant audit trails ([09. Security](09-security.md)).
- **No cross-tenant leakage** in the request inspector or live logs (filtered by tenant scope).

## Plugin & data isolation

- **Per-tenant plugin allow-lists & capabilities:** a tenant can load only approved plugins, with capability grants scoped to that tenant's resources (a tenant's plugin can reach only that tenant's allowed network/KV). WASM memory isolation already prevents cross-plugin/host access; tenant scoping adds capability boundaries ([12. Plugins](12-plugins.md)).
- **Data residency:** in multi-region/managed deployments, a tenant's certs/cache/state can be pinned to a region for compliance ([20. Future](20-future.md), [32. DR/HA](32-disaster-recovery-and-ha.md)).
- **Encryption scoping:** per-tenant encryption keys (KMS) for state-at-rest so a key compromise is tenant-bounded ([23. Data & State Model](23-data-and-state-model.md)).

## Blast-radius containment

The architecture limits how far any single failure or abuse spreads:
- A bad tenant **config** fails validation and never applies — and even an applied tenant change swaps only that tenant's portion of the snapshot (other tenants' routing is untouched).
- A tenant **quota breach, plugin trap, or upstream outage** is contained to that tenant by the per-tenant budgets and circuits above.
- A **compromised tenant admin token** is scoped to that tenant by RBAC and bounded by audit.
- **Strict mode** disables all cross-tenant sharing (separate cache/limit namespaces, no shared definitions) for tenants needing hard isolation; **shared mode** trades some isolation for efficiency where tenants are mutually trusting.

This containment is what lets a single Pulsate fleet safely serve thousands of independent domains/customers — the technical basis for the managed service and enterprise multi-tenancy.

## Cross-references
- [21. Threat Model](21-threat-model.md) — cross-tenant threats and mitigations.
- [09. Security](09-security.md) — RBAC, audit, secrets/encryption.
- [22. Admin API](22-admin-api.md) — scoped control-plane access.
- [08. Cache](08-cache.md) & [12. Plugins](12-plugins.md) — namespaced cache and per-tenant plugin sandboxing.
- [20. Future](20-future.md) — enterprise governance and the managed service.
