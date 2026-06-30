# Pulsate Security Audit

Date: 2026-06-30. Scope: all 26 `pulsate-*` crates. Method: adversarial read of every
security-sensitive surface (request path, admin auth, TLS/ACME, WAF, cache, WASM sandbox,
config, k8s controller) plus workspace-wide grep for `unsafe`/`unwrap`/`panic`/shell-out.

**Memory safety:** every crate declares `#![forbid(unsafe_code)]` — zero first-party `unsafe`.
**"No panics on the request path":** holds for network-reachable code.

Severity is for the default loopback deployment; cache/WAF/k8s items escalate when the admin
port or k8s controller is exposed.

---

## CRITICAL

### C1 — K8s Gateway API → Flow config injection (cross-tenant takeover + SSRF)
`crates/pulsate-k8s/src/translate.rs:294` (also :209, :216, :267)

Untrusted CRD string fields (`hostnames`, `HTTPPathMatch.value`, `method`, backend
`name`/`namespace`) are interpolated verbatim into Flow source text that is recompiled and
published cluster-wide. Only `upstream_name` is sanitized. `crd.rs` declares these as plain
`String` with no schema validation, so the API server accepts newlines and Flow metacharacters.

**Attack:** a tenant who can create an `HTTPRoute` in their own namespace sets a hostname like
`evil {\n tls off\n route /* ~> proxy(http://169.254.169.254/) }\nsite victim.com`. The
controller renders it and publishes a config rebuilt from *all* HTTPRoutes across *all*
namespaces → cross-tenant config takeover, SSRF to cloud metadata, `tls off` downgrade, route hijack.

**Fix:** build the typed `pulsate_config` model directly instead of round-tripping through text.
Failing that, strictly validate/escape every interpolated field (DNS-valid hosts, reject Flow
metacharacters/whitespace in path values, `[A-Z]`-only methods) and add CRD `pattern` validation.

---

## HIGH

### H2 — Default admin token derived from PID ⊕ wall-clock nanos (not a CSPRNG)
`crates/pulsate-cli/src/up.rs:268`

When `--admin-token` is omitted, the admin credential is `generate_token()` = FNV-1a over
`pid ^ start-time nanos`. Real entropy ≈ PID bits + guessable start time. The token grants
`config/reload` (arbitrary upstream re-pointing → traffic hijack). CRITICAL if admin is exposed.

**Fix:** fill 16–32 bytes from `OsRng`/`getrandom`, hex/base64 encode.

### H3 — Unbounded request-body buffering → memory-exhaustion DoS (data plane)
`crates/pulsate-http/src/serve.rs:109-112` (HTTP/1.1+H2, `body.collect()`),
`crates/pulsate-http3/src/server.rs:290-293` (H3 `BytesMut` append loop)

No size cap before dispatch. One large/slow POST × `max_connections` (10k) or H3
`max_concurrent_bidi_streams` (100) is fatal.

**Fix:** wrap bodies in `http_body_util::Limited` (track accumulated H3 len), return 413 past a
configurable cap.

### H4 — Cache key omits the query string → cache poisoning
`crates/pulsate-cache/src/lib.rs:144` + `pulsate-http/src/serve.rs:70`

Key is `method\nhost\npath`; query captured separately, never keyed. `/page?attacker` and
`/page?victim` share one entry → attacker primes content served to later visitors.

**Fix:** include the raw query / full request target in `CacheLayer::key`.

### H5 — Cache ignores upstream `Vary` → cross-user data disclosure
`crates/pulsate-cache/src/lib.rs:245-266`

`cacheable_ttl` checks status/`Set-Cookie`/`Cache-Control` but never reads response `Vary`. A
per-user response with `Vary: Authorization` (no `Set-Cookie`) is cached and served to another user.

**Fix:** honor response `Vary` — refuse to cache (or fold named request headers into the key)
when it names a header not already in the key.

### H6 — WASM sandbox sets no memory/table/instance limits
`crates/pulsate-plugin/src/lib.rs:80-85,128`

`Config` enables only `consume_fuel`; `Store::new` has no limiter. `memory.grow` costs ~1 fuel/call,
so a module grows linear memory toward 4 GiB within a tiny fuel budget → host OOM.

**Fix:** configure `StoreLimitsBuilder` (memory_size, table_elements, instances) via `store.limiter`.

---

## MEDIUM

- **M7 — IPv4-mapped IPv6 bypasses IP deny rules.** `crates/pulsate-waf/src/cidr.rs:17-31` + `lib.rs:236`. `::ffff:10.0.0.1` never matches `10.0.0.0/8`. Fix: `IpAddr::to_canonical()` before ACL/CIDR + rate-limit keying.
- **M8 — Allow-only IP ACL fails open.** `crates/pulsate-waf/src/lib.rs:236-244`. Configuring only `ip_allow` is a no-op (default-allow). Fix: deny by default when any allow rule exists and IP matches none.
- **M9 — WAF matching trivially bypassable.** `crates/pulsate-waf/src/lib.rs:182-202`. Case-folded substring on single-decoded path+query only; `union/**/select`, `%09`, `%252e` evade; no header/body inspection. (No regex → no ReDoS.) Fix: canonicalize to fixed point, extend coverage, or document as path-only defense-in-depth.
- **M10 — Audit-log hash chain is keyless 64-bit FNV-1a (not tamper-evident).** `crates/pulsate-waf/src/lib.rs:328`. Anyone editing an entry recomputes downstream hashes; `verify()` still passes. Fix: HMAC-SHA256 keyed with a server secret.
- **M11 — Admin bearer token printed to stdout.** `crates/pulsate-cli/src/up.rs:119`. Lands in journald/Docker/k8s logs. Fix: print a fingerprint or write to a `0600` file.
- **M12 — `Connection`-named hop headers not stripped.** `crates/pulsate-proxy/src/forward.rs:23-34`. `Connection: X-Internal-Auth` keeps that header forwarded (RFC 9110 §7.6.1). Fix: parse the `Connection` value, strip each named token.
- **M13 — Unbounded cache memory (count-only cap).** `crates/pulsate-cache/src/lib.rs:202-241`. 10k multi-MB bodies → multi-GB OOM. Fix: per-body + total-byte budget with size-aware LRU.

---

## LOW / INFORMATIONAL

- Non-constant-time token compare — `pulsate-control/src/lib.rs:149` (`HashMap::get`). Auth coverage on all `/v1`+gRPC routes verified complete; SipHash makes timing impractical, but store HMAC + `constant_time_eq`.
- Secrets as plain `String`, no zeroization — `pulsate-secrets/src/lib.rs:39,78`. Wrap in `zeroize`/`secrecy`.
- CORS `*` + `credentials=true` — `pulsate-pipeline/src/lib.rs:205,220`. Spec-invalid; reject at config-compile.
- XFF appended, inbound preserved — `pulsate-proxy/src/forward.rs:162-169`. Offer a "reset XFF" mode.
- `${ENV}` expansion in upstream targets — `pulsate-config/src/compile.rs:692-698`. Flow author can read any process env var.
- WASM fuel-only (no epoch deadline) + capability denial by error-string match — `plugin/lib.rs:79-85,150`. Add epoch interruption; pre-scan `module.imports()`.
- Flow parser indexing/`unreachable!` — `pulsate-flow/src/parser.rs:38,46,152`. Config-only, not network-reachable; harden the empty-token case.

---

## Audited and confirmed solid

- No first-party `unsafe` (all 26 crates `#![forbid(unsafe_code)]`).
- TLS/ACME: rustls 0.23 (`tls12`+`ring`), no SSLv3/TLS1.0/1.1, no dangerous verifiers; ACME `allow_any()` unused; `AccountKey` redacts in `Debug`.
- Request smuggling (CL/TE, H2/H3→1.1 downgrade): proxy fully buffers + re-emits via `Full<Bytes>` with fresh Content-Length; framing headers hop-by-hop.
- Path traversal: `handlers.rs::sanitized_join` rejects ParentDir/RootDir/Prefix; dashboard assets `include_str!`-compiled; secret `FileBackend` rejects `/` and `..`.
- Percent-decode bound (`i+2 < len`) correct; LB modulo guarded against div-by-zero.
- Rate-limit keying uses real TCP peer IP, not XFF (no spoof bypass).
- Config parsing: custom `pulsate_flow` parser (no serde_yaml billion-laughs); migration is pure text→text.
- No shell-out anywhere in scope.
