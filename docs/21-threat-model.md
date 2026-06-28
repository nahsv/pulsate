# 21. Threat Model

> A STRIDE-based threat model for Pulsate: the trust boundaries, the attack surface, the adversaries and their abuse cases, and the mitigations — mapped to the controls in [09. Security](09-security.md). Security through transparency.

**Contents**
- [Scope & assumptions](#scope--assumptions)
- [Trust boundaries](#trust-boundaries)
- [Assets](#assets)
- [STRIDE analysis](#stride-analysis)
- [Attack surface & abuse cases](#attack-surface--abuse-cases)
- [Threat → mitigation matrix](#threat--mitigation-matrix)
- [Residual risk & assurance](#residual-risk--assurance)
- [Cross-references](#cross-references)

---

## Scope & assumptions

Pulsate sits at the edge: it terminates untrusted client traffic and forwards to (more) trusted backends. The model assumes a hostile internet, possibly-hostile tenants in shared deployments, and untrusted plugin code. It covers the data plane, control plane, admin surface, plugin sandbox, and state at rest. It assumes the host OS and kernel are trusted-but-hardened and that operators follow the deployment guidance ([16. Deployment](16-deployment.md)).

## Trust boundaries

```
   UNTRUSTED                    │ semi-trusted │        TRUSTED (operator-controlled)
                                │              │
 ┌─────────┐   TLS    ┌─────────▼───┐  ┌───────▼────────┐   ┌──────────────┐
 │ Internet│ ───────▶ │ Pulsate DATA  │  │ Pulsate CONTROL  │   │  Upstreams   │
 │ clients │          │ plane (edge)│  │ plane + Admin  │   │ (backends)   │
 └─────────┘          └──────┬──────┘  └───────┬────────┘   └──────────────┘
      ▲                      │ snapshot         │ admin API (authn/RBAC)
      │                ┌─────▼──────┐    ┌──────▼───────┐    ┌──────────────┐
   WASM plugin ◀── sandbox boundary │    │ State store  │    │ Secrets/KMS  │
   (untrusted code)  └────────────┘     │ (certs/cache)│    │ ACME CA / DNS│
                                        └──────────────┘    └──────────────┘
   Boundaries crossed: (1) client↔data-plane, (2) data↔control (snapshot only),
   (3) operator↔admin-API, (4) Pulsate↔plugin (sandbox), (5) Pulsate↔upstream,
   (6) Pulsate↔state-at-rest, (7) tenant↔tenant (multi-tenant).
```

Each boundary is an authorization/validation checkpoint. The strongest invariant: **the data plane only ever reads an immutable, already-validated snapshot** — untrusted input cannot reach control-plane logic synchronously ([02. Architecture](02-architecture.md)).

## Assets

Private keys & ACME account keys; TLS session/ticket keys; secrets (upstream creds, API tokens); configuration (and its integrity); cached content (confidentiality/integrity); audit logs (integrity); the admin API (full control); availability of the edge itself; and tenant isolation in shared mode.

## STRIDE analysis

| STRIDE | Threat to Pulsate | Primary mitigations |
|---|---|---|
| **Spoofing** | client impersonation; forged `X-Forwarded-For`/identity headers; UA-spoofed "good bots"; rogue cluster node | mTLS/JWT identity; trust forwarded headers only from configured trusted proxies; verified-crawler reverse-DNS; cluster mutual auth ([09. Security](09-security.md)) |
| **Tampering** | request smuggling; response/cache poisoning; config tampering; in-transit modification | strict framing (reject ambiguous CL/TE); cache-key/Vary correctness; signed/audited config changes; TLS everywhere; state encrypted at rest |
| **Repudiation** | "I didn't change that"; untraceable abuse | tamper-evident hash-chained audit log (actor+diff); request IDs tying actions to traces/logs ([15. Observability](15-observability.md)) |
| **Information disclosure** | key/secret leakage; version/banner leakage; cache serving private data to wrong user; side channels | secrets redacted everywhere, encrypted at rest, never logged; `server`/`x-powered-by` stripped; cache never shares `private`/auth'd responses without identity key; constant-time crypto via rustls |
| **Denial of service** | slowloris; H2 rapid-reset/floods; QUIC amplification; cache stampede; retry storms; ReDoS; algorithmic complexity | per-phase timeouts; H2/H3 frame/stream-churn limits; QUIC anti-amplification + retry tokens; single-flight cache; retry budgets + breakers; regex/rule complexity bounds; connection/rate limits |
| **Elevation of privilege** | plugin escaping sandbox; admin-API abuse; privilege not dropped; SSRF via proxy to internal | WASM memory+capability sandbox, fuel/epoch, deny-by-default; admin loopback + authn/RBAC + audit; drop privileges after bind; SSRF guards (upstream allow-lists, block link-local/metadata IPs) |

## Attack surface & abuse cases

- **Client-facing data plane** (largest surface): malformed/oversized requests, protocol abuse (H1 smuggling, H2 rapid-reset, H3/QUIC amplification), TLS downgrade attempts, header/host confusion, ReDoS via crafted inputs to regex routes/WAF. → strict parsing, fuzzing, bounded everything, protocol-specific DoS mitigations ([05. HTTP Stack](05-http-stack.md)).
- **Admin API/dashboard:** the keys-to-the-kingdom surface. → loopback-by-default, mandatory authn + RBAC when exposed, audit every action, rate-limit auth, CSRF protection on the dashboard, short-lived tokens.
- **Plugin/supply chain:** a malicious or vulnerable plugin; a compromised plugin registry; dependency compromise. → sandbox + capability transparency + optional mandatory signing/verification; SBOM + `cargo-deny`/audit; reproducible builds ([33. Release Engineering](33-release-engineering-and-supply-chain.md), [12. Plugins](12-plugins.md)).
- **Certificate/ACME:** domain-validation abuse, on-demand issuance abuse (cert-mining), DNS-provider credential theft. → on-demand allow-listing, CA rate-limit awareness, DNS creds via secrets backend, CAA respect.
- **State at rest:** theft of the data dir (keys, cache, audit). → encryption at rest (KMS-backed), strict file permissions, secrets never persisted in plaintext ([23. Data & State Model](23-data-and-state-model.md)).
- **Multi-tenant:** noisy-neighbor DoS, cross-tenant data/config access, cache key collisions. → per-tenant quotas, namespaced config/cache keys, RBAC scoping ([29. Multi-Tenancy](29-multi-tenancy-and-isolation.md)).
- **SSRF via the proxy:** tricking Pulsate into requesting internal/metadata endpoints. → upstreams are explicitly configured (not client-controlled); when discovery/redirects are involved, block private/link-local/metadata ranges by default.

## Threat → mitigation matrix

| Abuse case | Likelihood | Impact | Mitigation owner |
|---|---|---|---|
| HTTP/2 rapid-reset flood | High | High (DoS) | `pulsate-http` stream-churn budget; [05](05-http-stack.md) |
| Request smuggling | Med | High (bypass/poison) | strict framing; fuzz; [05](05-http-stack.md) |
| Cache poisoning via unkeyed header | Med | High | key/Vary correctness; lint; [08](08-cache.md) |
| Exposed unauthenticated admin API | Med | Critical | loopback default + authz + startup warning; [09](09-security.md)/[22](22-admin-api.md) |
| Malicious plugin exfiltration | Med | High | capability deny-by-default + signing; [12](12-plugins.md) |
| Secret leakage in logs | Med | High | redaction everywhere; [09](09-security.md) |
| ACME on-demand cert mining | Low | Med (rate-limit ban) | allow-list hosts; [09](09-security.md) |
| QUIC reflection/amplification | Med | Med | anti-amplification limits + retry token; [05](05-http-stack.md) |
| Retry storm collapsing backend | Med | High | retry budgets + breakers; [06](06-reverse-proxy.md) |
| Cross-tenant access (shared) | Low | High | namespacing + RBAC + quotas; [29](29-multi-tenancy-and-isolation.md) |

## Residual risk & assurance

- **Defense in depth:** no single control is trusted alone (e.g., DoS resisted at connection limits, timeouts, rate limits, and breakers).
- **Assurance practices:** continuous fuzzing of parsers/decoders; protocol conformance ([28. Testing](28-testing-and-conformance.md)); a pre-1.0 **external security audit** plus ongoing review ([19. Milestones](19-milestones.md)); a public security policy and coordinated disclosure ([18. Open Source](18-open-source.md)).
- **Known residuals:** kernel/OS and hardware side-channels are out of Pulsate's control (rely on platform hardening); a determined supply-chain attack on a transitively-trusted dependency is mitigated, not eliminated (SBOM + pinning + review reduce, not zero, the risk).
- **Secure defaults as the backstop:** because the safe configuration is the default ([09. Security](09-security.md)), the most common real-world failure — misconfiguration — is minimized.

## Cross-references
- [09. Security](09-security.md) — the controls implementing these mitigations.
- [05. HTTP Stack](05-http-stack.md) — protocol-level DoS/abuse defenses.
- [12. Plugins](12-plugins.md) — the sandbox boundary and supply-chain controls.
- [22. Admin API](22-admin-api.md) — admin surface authz model.
- [29. Multi-Tenancy](29-multi-tenancy-and-isolation.md) — tenant isolation.
- [33. Release Engineering & Supply Chain](33-release-engineering-and-supply-chain.md) — build/distribution integrity.
