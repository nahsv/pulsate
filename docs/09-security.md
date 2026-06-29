# 09. Security

> Secure by default in detail: TLS posture and certificate management, the WAF, rate limiting, bot/geo/ASN controls, JWT and mTLS, security headers, secrets management, and audit logging — and the defaults that make Pulsate safe before you configure anything.

**Contents**
- [Security posture & secure defaults](#security-posture--secure-defaults)
- [TLS](#tls)
- [Certificate management](#certificate-management)
- [WAF](#waf)
- [Rate limiting](#rate-limiting)
- [Bot detection](#bot-detection)
- [Geo & ASN blocking](#geo--asn-blocking)
- [JWT & token auth](#jwt--token-auth)
- [mTLS](#mtls)
- [Security headers](#security-headers)
- [Secrets management](#secrets-management)
- [Audit logging](#audit-logging)
- [Cross-references](#cross-references)

---

## Security posture & secure defaults

Pulsate ships **safe before configured**. Out of the box, with a minimal config:
- TLS is automatic; HTTP redirects to HTTPS; HSTS is set.
- TLS ≥ 1.2; weak ciphers/protocols disabled; secure cipher preset.
- The admin API and dashboard bind to **loopback only**.
- Sensible security response headers are applied (HSTS, `X-Content-Type-Options`, frame options, referrer policy).
- Request limits (header sizes, body caps, connection/stream limits) are set to bound abuse.
- Forwarded headers from untrusted sources are **not** trusted.
- No request smuggling: ambiguous framing rejected.

You opt *out* of protections explicitly; you never have to discover and enable them. The complete adversary analysis lives in [21. Threat Model](21-threat-model.md); this document specifies the controls.

## TLS

(See [05. HTTP Stack — TLS](05-http-stack.md#tls) for protocol mechanics.) Security-relevant policy:
- **Versions:** TLS 1.3 preferred, 1.2 allowed; 1.0/1.1 off. QUIC mandates 1.3.
- **Cipher presets:** `modern` (1.3 AEAD suites only), `intermediate` (broad client compat), or an explicit allow-list. ECDHE key exchange; no static RSA key exchange.
- **HSTS** on by default for ACME-secured sites (`max-age` + `includeSubDomains`, opt-in `preload`).
- **OCSP stapling** enabled; must-staple supported.
- **Session resumption** uses rotated ticket keys (forward-secrecy-preserving), shared across a cluster so resumption survives node changes.
- **FIPS mode:** an optional build (`--features fips`) uses a FIPS-validated crypto provider for regulated environments ([29. Multi-Tenancy](29-multi-tenancy-and-isolation.md), [20. Future](20-future.md)).

## Certificate management

`pulsate-acme` (control plane) owns certificates end to end:
- **Issuance** via ACME (Let's Encrypt/ZeroSSL/custom directory) with HTTP-01, TLS-ALPN-01, or DNS-01 (wildcards). On-demand issuance for dynamically-appearing hostnames is supported with allow-listing to prevent abuse (only issue for hosts you actually serve).
- **Renewal** at ~⅔ lifetime, with retry/backoff and alerting on failure; the old cert stays valid until the new one is atomically installed (no gap).
- **Storage:** certs, keys, and ACME account keys are persisted (encrypted at rest) in the state store and **shared across cluster nodes** so a multi-node fleet issues once, not per node ([23. Data & State Model](23-data-and-state-model.md)).
- **Manual certs & external CAs:** file-based certs, and integration with secret backends/KMS for keys (`key secret://...`).
- **Visibility:** certificate inventory, expiry, issuer, and renewal status are exposed on the dashboard's [certificate manager](11-dashboard.md) and via the [22. Admin API](22-admin-api.md), with metrics/alerts for near-expiry.

## WAF

`pulsate-waf` provides a layered web application firewall as middleware (`waf(@ruleset)`):

- **Rule engine:** evaluates ordered rules against request attributes (method, path, headers, query, body up to a bounded inspection size). Supports a built-in **OWASP CRS-compatible** ruleset plus custom rules.
- **Modes:** `block` (reject matches) or `detect` (log only — for tuning before enforcing). Per-rule severity and actions (block/challenge/log/tag).
- **Detections:** common injection classes (SQLi, XSS, path traversal, command injection, SSRF patterns), protocol anomalies, and oversized/malformed inputs.
- **Custom rules** in a readable rule format (`/etc/pulsate/waf/*.rules`), hot-reloadable.
- **Performance:** rules compile to efficient matchers; body inspection is bounded and streamed; the WAF runs early in [Ingress] so blocked requests cost little.
- **Tuning:** per-route rule exclusions, anomaly scoring with a configurable threshold (CRS-style), and false-positive reporting from the dashboard.

The full custom-rule grammar and ruleset management are in [27. Configuration Reference](27-config-reference.md); abuse cases in [21. Threat Model](21-threat-model.md).

## Rate limiting

`rate_limit` middleware (with `pulsate-waf` backing) protects against floods and enforces fair use:

```
route /* ~> rate_limit(1000/min, key=ip, burst=200)
route /login ~> rate_limit(5/min, key=[ip, path])      # stricter on sensitive endpoints
route /api/* ~> rate_limit(10000/h, key=header.x-api-key)
```

- **Algorithms:** token bucket (with burst) and sliding-window log/counter; choose per limiter.
- **Keys:** composite — IP, header, cookie, JWT claim, path, or any combination; IP keys respect trusted-proxy resolution so you limit the real client, not your edge.
- **Distributed mode:** counters can be backed by Redis/cluster for fleet-wide limits (with a local fast-path approximation to avoid a network hop per request).
- **Responses:** `429` with `Retry-After` and `RateLimit-*` headers (draft standard) so clients can self-throttle.
- **Tiers:** different limits per consumer tier (free/pro/enterprise) via the key + a limit table — useful for API products ([29. Multi-Tenancy](29-multi-tenancy-and-isolation.md)).

## Bot detection

```
waf w { bot { mode challenge; allow [googlebot, bingbot]; deny [badscraper] } }
```

- **Signals:** user-agent analysis, known-bot allow/deny lists (verified via reverse-DNS for declared crawlers), request-fingerprint heuristics, and behavioral signals (rate/pattern).
- **Actions:** `allow`, `block`, or `challenge` (a lightweight proof-of-work or JS challenge, or integration with a CAPTCHA provider via plugin).
- **Verified crawlers:** legitimate search bots are verified (forward+reverse DNS) before being allow-listed, preventing UA spoofing.
- Bot policy integrates with rate limiting and the WAF score.

## Geo & ASN blocking

```
waf w {
  geo { block [KP, RU]; allow [] }      # ISO 3166 country codes
  asn { block [AS13335]; allow [AS15169] }
}
```

- **GeoIP/ASN** lookups via a MaxMind-format database (`maxminddb`), kept updatable; the DB path is configured and can be refreshed without restart.
- **Allow-list vs block-list** semantics are explicit (allow-list wins; an empty allow-list means "all except blocked").
- Decisions feed the same policy pipeline as WAF/bot, are logged, and are exposed as metrics (`pulsate_waf_blocked_total{reason=geo|asn|bot|rule|ip}`).

## JWT & token auth

```
route /api/* ~> jwt(
  iss  = "https://issuer.example.com",
  aud  = "api",
  jwks = "https://issuer.example.com/.well-known/jwks.json",   # auto-refreshed, cached
  algorithms = [RS256, ES256],
  leeway = 60s,
  forward_claims = [sub, scope]        # inject claims as headers to upstream
) ~> require(scope contains "read") ~> proxy(@api)
```

- **Validation:** signature (JWKS auto-fetched/rotated, or static keys), `exp`/`nbf`/`iat` with leeway, `iss`/`aud`, and required claims. Algorithm allow-list prevents `alg=none`/downgrade attacks.
- **Authorization:** the `require(expr)` middleware evaluates a claims/cert expression (`scope contains "admin"`, `cert.cn in [...]`).
- **Forwarding:** selected verified claims are injected as trusted headers to the upstream (and conflicting client-supplied headers are stripped to prevent spoofing).
- **Other schemes:** `basic_auth` (with secret-stored hashes), API keys, and `forward_auth` (delegate the decision to an external service via subrequest) are all built in.

## mTLS

- **Downstream (client → Pulsate):** `client_auth { mode request|require; ca <bundle> }` verifies client certs; verified identity (CN/SAN/SPIFFE ID) is exposed to `require(...)` and logged. CRL/OCSP revocation checking supported.
- **Upstream (Pulsate → backend):** present a client certificate to backends (`tls { client_cert, client_key, ca }`) for zero-trust internal traffic.
- **SPIFFE/SVID** identities are recognized for service-mesh-adjacent deployments.

## Security headers

Applied by default from `defaults { headers { ... } }`, overridable per route:

| Header | Default |
|---|---|
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` (on ACME sites) |
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` / CSP `frame-ancestors` | `DENY` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `Content-Security-Policy` | opt-in template (off by default to avoid breaking apps, with a guided generator) |
| `Permissions-Policy` | minimal sensible default |

The `server`/`x-powered-by` headers are stripped by default (no version leakage).

## Secrets management

`pulsate-secrets` resolves `secret://name` references so credentials never sit in the config file:
- **Backends:** environment, mounted file/dir, HashiCorp Vault, and cloud KMS/secret managers (AWS/GCP/Azure), plus plugin backends.
- **Resolution:** secrets are fetched at load and on rotation; references are resolved into memory only, never written back to disk or logs.
- **Rotation:** backends that support leases/rotation trigger a snapshot rebuild (e.g., a rotated upstream credential or TLS key) without downtime.
- **Redaction:** secret values are redacted in logs, the dashboard, audit records, and `pulsate config dump`.
- **Encryption at rest:** the local state store (certs/keys) is encrypted with a key from the secrets backend/KMS.

## Audit logging

A tamper-evident record of *who changed what* and *security-relevant events*:
- **Config changes:** every reload/apply records the actor (CLI user, admin-API token, K8s controller), a diff/hash of old→new config, timestamp, and source.
- **Security events:** auth failures, WAF blocks, rate-limit trips, cert issuance/renewal, privilege drops, admin-API access.
- **Format:** structured JSON to a dedicated audit sink (file, syslog, or OTLP), separate from access logs, with optional **hash-chaining** (each record includes the prior record's hash) so tampering is detectable.
- **Retention & export** are configurable; audit logs feed SIEMs and satisfy compliance needs ([29. Multi-Tenancy](29-multi-tenancy-and-isolation.md)).

## Cross-references
- [21. Threat Model](21-threat-model.md) — STRIDE analysis, trust boundaries, abuse cases.
- [05. HTTP Stack](05-http-stack.md) — TLS/QUIC mechanics and protocol abuse defenses.
- [04. Configuration](04-configuration.md) — `waf`, `tls`, `jwt`, `rate_limit`, `headers` syntax.
- [23. Data & State Model](23-data-and-state-model.md) — encrypted cert/secret storage.
- [12. Plugins](12-plugins.md) — sandboxing of third-party code as a security boundary.
