# 20. Future

> Beyond 1.0: the commercial and ecosystem arc — an enterprise edition, a cloud control plane, a managed service, commercial support, the plugin ecosystem, AI-assisted diagnostics, and multi-region deployments — and the principles that keep these additive to a thriving open-source core.

**Contents**
- [Guiding principle: additive, never extractive](#guiding-principle-additive-never-extractive)
- [Enterprise edition](#enterprise-edition)
- [Cloud control plane](#cloud-control-plane)
- [Managed service](#managed-service)
- [Commercial support](#commercial-support)
- [Plugin ecosystem](#plugin-ecosystem)
- [AI-assisted diagnostics](#ai-assisted-diagnostics)
- [Multi-region deployments](#multi-region-deployments)
- [Sequencing & how it funds the core](#sequencing--how-it-funds-the-core)
- [Cross-references](#cross-references)

---

## Guiding principle: additive, never extractive

Every item here is **additive** to the open-source core, never a clawback. The open/closed boundary is a published promise ([18. Open Source](18-open-source.md)): single-node and basic multi-node Pulsate — auto-TLS, cache, WAF, LB, observability, plugins, dashboard, clustering — stay fully open and production-grade forever. Commercial offerings target *fleet-scale operations, managed convenience, and enterprise governance* — value a single team running a single cluster doesn't need. This is what makes the open core trustworthy and the business durable.

## Enterprise edition

A separately-licensed (BSL/commercial) build/add-on for organizations with compliance and scale needs the OSS core intentionally doesn't carry:
- **Advanced governance:** fine-grained RBAC, SSO/SAML/SCIM, policy-as-code guardrails, approval workflows for config changes, and per-tenant quotas at fleet scale ([29. Multi-Tenancy](29-multi-tenancy-and-isolation.md)).
- **Compliance:** signed compliance attestations (SOC 2, ISO 27001), a **FIPS-validated crypto build**, data-residency controls, and extended tamper-evident audit with SIEM connectors.
- **Hardened operations:** long-term-support (LTS) lines, hotfix backports, air-gapped install + offline plugin registry, and certified reference architectures.
- **Premium integrations:** enterprise secret managers/HSMs, identity providers, and ticketing/observability suites.
These are built on the same engine; the enterprise edition is configuration, governance, and support — not a fork of the data plane.

## Cloud control plane

A **multi-cluster control plane** (self-hosted or SaaS) that manages a fleet of open-source Pulsate data planes — the optional external counterpart to the in-process control plane ([02. Architecture](02-architecture.md)):
- **Centralized config & policy:** author once, validate, canary, and roll out `pulsate.flow`/policy across regions and clusters, with diff/preview and coordinated rollback — GitOps-native.
- **Fleet certificate management:** issue/renew/rotate across the fleet from one place, with global visibility and expiry SLAs.
- **Unified observability:** aggregate metrics/traces/logs and per-route SLOs across all nodes; fleet-wide cache-purge and rate-limit policy.
- **Design stance:** the data plane stays the OSS binary and remains fully functional if the control plane is unreachable (the control plane configures, it is not in the request path) — preserving Pulsate's "self-sufficient node" invariant.

## Managed service

**Pulsate Cloud** — a fully-managed, usage-priced edge/gateway:
- Bring a domain and an origin; Pulsate Cloud runs the data plane (globally distributed PoPs), handles TLS, caching, WAF, and DDoS posture, and exposes the same Flow config and dashboard — the "Vercel/Cloudflare-simplicity" tier for teams who don't want to run infrastructure.
- **Edge network:** anycast PoPs with global cache coherence (builds on [multi-region](#multi-region-deployments)).
- **Onramp continuity:** the same `pulsate.flow` runs on your laptop, your cluster, and Pulsate Cloud — no rewrite to adopt or to leave (anti-lock-in is a feature, and a trust-builder).

## Commercial support

- **Tiered support** (business/enterprise) with response-time SLAs, a named TAM at the top tier, and guided onboarding/migration.
- **Professional services:** migration from nginx/Envoy/Traefik at scale, performance engineering, custom plugin development, and architecture review.
- **Training & certification** for operators and plugin developers; a partner/reseller program.
Support revenue funds core maintenance regardless of edition mix.

## Plugin ecosystem

Grow the WASM plugin model ([12. Plugins](12-plugins.md)) into a network effect:
- **Marketplace:** a curated registry of community and verified plugins with up-front **capability transparency** (you see exactly what a plugin can access before installing), ratings, signing/provenance, and one-line install from OCI.
- **Revenue share** for commercial plugin authors; a "verified publisher" program for trust.
- **Richer extension surfaces** over time: more host capabilities (carefully, capability-gated), more languages with first-class SDKs, and composability (plugin pipelines).
- **Certified plugins** for enterprise (vetted, supported) bridging OSS extensibility and enterprise assurance.

## AI-assisted diagnostics

The per-stage [request lifecycle](02-architecture.md#request-lifecycle) telemetry and structured errors are the substrate for an **operator co-pilot**:
- **"Why is this slow / 502?"** — point the assistant at a request ID or a route; it reads the per-stage spans, upstream health, breaker state, and recent config diffs to produce a ranked root-cause and a suggested fix ("upstream `@api` p99 tripled after the 14:02 reload that changed the timeout; revert or raise `upstream.response`").
- **Config intelligence:** natural-language → validated Flow snippets, anti-pattern detection beyond static lint ("this route caches authenticated responses without an identity key"), and migration assistance.
- **Adaptive security:** anomaly-based WAF tuning and rate-limit recommendations from observed traffic, surfaced as suggestions an operator approves (human-in-the-loop, never silent enforcement).
- **Capacity & cost:** forecast capacity and recommend autoscaling/cache-sizing from trends.
Delivered as an opt-in feature (local or cloud-backed), respecting data-privacy controls; it advises, the operator decides.

## Multi-region deployments

- **Active-active edge:** geo-distributed Pulsate nodes with anycast/GeoDNS routing users to the nearest PoP, health-aware failover across regions, and latency-based origin selection.
- **Global cache coherence:** tiered caching with cross-region invalidation (a purge is global in seconds) and regional origin shielding to minimize origin load.
- **Global state:** certificates, rate-limit budgets, and config replicated across regions with clear consistency semantics (strong where required, eventual where it's cheaper), building on [32. Disaster Recovery & HA](32-disaster-recovery-and-ha.md).
- **Traffic management:** progressive global rollouts, traffic shadowing/mirroring to a new region, and fault injection for resilience testing.

## Sequencing & how it funds the core

Roughly mapping to the [01. Vision](01-vision.md) Year 3–5 arc:
1. **Y3:** plugin marketplace + commercial support + enterprise governance (lowest build cost, immediate enterprise pull).
2. **Y4:** cloud control plane + enterprise edition (FIPS/compliance) — fleet customers.
3. **Y5:** managed service + multi-region edge + AI diagnostics — the intelligent-edge vision.

The business model (open-core + managed + support) is chosen so that **commercial success requires the open core to thrive** — every paying customer runs the OSS data plane, so we are structurally incentivized to keep it excellent. No feature that exists in the open core is ever moved behind the paywall.

## Cross-references
- [01. Vision](01-vision.md) — the 5-year roadmap this realizes.
- [18. Open Source](18-open-source.md) — the open-core boundary and license.
- [02. Architecture](02-architecture.md) — why an external control plane stays out of the request path.
- [29. Multi-Tenancy](29-multi-tenancy-and-isolation.md) — the isolation the enterprise/managed tiers build on.
- [32. Disaster Recovery & HA](32-disaster-recovery-and-ha.md) — multi-region foundations.
