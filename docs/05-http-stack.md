# 05. HTTP Stack

> The wire: how Pulsate terminates HTTP/1.1, HTTP/2, and HTTP/3 over TLS/QUIC, how it handles WebSocket, SSE, gRPC, and streaming, and how connection pooling, keep-alive, timeouts, buffers, and zero-copy are engineered.

**Contents**
- [Protocol matrix](#protocol-matrix)
- [HTTP/1.1](#http11)
- [HTTP/2](#http2)
- [HTTP/3 & QUIC](#http3--quic)
- [TLS](#tls)
- [ACME](#acme)
- [WebSocket](#websocket)
- [Server-Sent Events](#server-sent-events)
- [gRPC](#grpc)
- [Streaming](#streaming)
- [Connection pooling & keep-alive](#connection-pooling--keep-alive)
- [Timeouts](#timeouts)
- [Buffer management](#buffer-management)
- [Zero-copy opportunities](#zero-copy-opportunities)
- [Cross-references](#cross-references)

---

## Protocol matrix

| Concern | HTTP/1.1 | HTTP/2 | HTTP/3 |
|---|---|---|---|
| Transport | TCP | TCP | QUIC (UDP) |
| Library | hyper (`pulsate-http`) | hyper/h2 (`pulsate-http`) | quinn + h3 (`pulsate-http3`) |
| Multiplexing | 1 req/conn + keep-alive | streams | streams (no HoL blocking) |
| Negotiation | default / Upgrade | ALPN `h2` | Alt-Svc + ALPN `h3` |
| Server push | n/a | not used (deprecated) | n/a |
| Flow control | TCP | HTTP/2 window | QUIC + HTTP/3 |

All three normalize into the same internal `Request`/`Response` model in `pulsate-core`, so routing, middleware, proxy, and cache are protocol-agnostic. Protocol-specific behavior is confined to the codec crates.

## HTTP/1.1

- Built on **hyper** server connections. One request is served at a time per connection with **keep-alive**; pipelining is accepted but responses are serialized (the safe, standard behavior).
- **Strict parsing:** header count/size limits, request-line length limits, and rejection of ambiguous framing (e.g., conflicting `Content-Length`/`Transfer-Encoding`) to prevent request smuggling. Malformed input → `400` with a stable error code, never undefined behavior (memory-safe by construction).
- **Upgrade** handling routes `Connection: Upgrade` to the WebSocket handler or to a raw tunnel where configured.
- **Expect: 100-continue** is honored: Pulsate sends `100 Continue` only after Ingress middleware/auth approve the request, avoiding wasted upload bandwidth on rejected requests.

## HTTP/2

- ALPN-negotiated `h2` over TLS (h2c cleartext is supported only for explicit internal listeners). Each stream becomes its own request task; the connection task multiplexes frames.
- **Concurrency & flow control:** `max_concurrent_streams`, initial window sizes, and frame size are configurable; sensible secure defaults cap concurrent streams to bound memory.
- **Abuse defenses:** mitigations for known HTTP/2 DoS classes (rapid-reset / CVE-2023-44487-style, settings floods, ping floods, header-list bombs) — per-connection stream-churn budgets and frame-rate limits that reset/close abusive connections. Tied into [09. Security](09-security.md) and [21. Threat Model](21-threat-model.md).
- **Trailers** are supported (needed for gRPC).

## HTTP/3 & QUIC

- **quinn** provides QUIC; **h3** provides the HTTP/3 mapping. QUIC runs over UDP with TLS 1.3 built into the handshake.
- Advertised via **`Alt-Svc`** from the HTTP/2/1.1 responses so clients upgrade automatically; the HTTPS listener opens a paired UDP socket on the same port.
- **0-RTT** is supported but **disabled by default for non-idempotent requests** (replay risk); enabling 0-RTT for safe methods is opt-in.
- **Connection migration** (client IP/port changes) is handled by QUIC connection IDs, valuable for mobile clients.
- **UDP performance:** GSO/GRO and batched send/recv (`sendmmsg`/`recvmmsg`) where the OS supports them, to keep UDP throughput competitive with TCP.

## TLS

- **rustls** only — no OpenSSL — keeping the one-binary, memory-safe promise. TLS 1.3 and 1.2; 1.0/1.1 disabled by default.
- **SNI-based certificate selection** from the snapshot's cert view; wildcard and SAN matching; a default cert for unmatched SNI.
- **ALPN** negotiates `h2`/`http/1.1` (TCP) and `h3` (QUIC).
- **Session resumption** via TLS 1.3 tickets and a session cache, reducing handshake cost; ticket keys are rotated and (in a cluster) shared so resumption works across nodes.
- **OCSP stapling** is fetched and refreshed by the control plane and stapled by the data plane.
- **mTLS:** client-certificate `request`/`require` modes, CA bundle verification, and exposure of verified client identity (CN/SAN) to middleware for authorization (`require(cert.cn in [...])`).
- **Cipher policy** presets: `modern` (1.3-only suites), `intermediate` (broad compatibility), or an explicit list. Crypto provider is pluggable to allow a **FIPS-validated** build ([20. Future](20-future.md), [36-style compliance in 29/21]).

## ACME

- **instant-acme** drives certificate issuance/renewal in `pulsate-acme` (control plane).
- Challenge types: **HTTP-01** (served on the port-80 listener), **TLS-ALPN-01** (served on 443 via a special ALPN cert), and **DNS-01** (via pluggable DNS providers — required for wildcards).
- **Lifecycle:** on first need, request a cert; renew at ~⅔ of lifetime; retry with backoff on failure; keep the old cert valid until the new one is installed (atomic cert-view swap, like config). Certs and ACME account keys are persisted in the [state store](23-data-and-state-model.md) and shared across a cluster so nodes don't each solve challenges.
- **Rate-limit awareness:** Pulsate respects CA rate limits and uses staging directories in tests (Pebble in CI — [28. Testing](28-testing-and-conformance.md)).

## WebSocket

- The `ws(@upstream)` handler (or automatic Upgrade detection) bridges a client WebSocket to an upstream WebSocket, proxying frames bidirectionally with backpressure.
- Works over HTTP/1.1 Upgrade and HTTP/2 (`CONNECT`/Extended CONNECT, RFC 8441). Idle and lifetime timeouts apply; ping/pong keepalive is passed through (and optionally injected).
- Middleware (auth, rate limit, WAF) runs on the **handshake** request before the socket is established, so WebSockets are governed by the same policy as HTTP routes.

## Server-Sent Events

- SSE is plain HTTP with a streaming `text/event-stream` body; Pulsate detects it and **disables response buffering and compression-by-default for that content type**, flushing events immediately so they are not coalesced.
- Long-lived SSE connections are exempt from the normal response-idle timeout (configurable) but still subject to a max-lifetime cap.

## gRPC

- The `grpc(@upstream)` handler proxies gRPC (HTTP/2 with trailers) end-to-end, preserving `grpc-status`/`grpc-message` trailers and bidirectional streaming.
- **gRPC-Web** translation (HTTP/1.1 ↔ gRPC) is available so browser clients can reach gRPC backends.
- Per-method routing via the `~ ^/package.Service/Method$` path matcher; load balancing is **per-request** (L7), correctly spreading streams across backends rather than pinning a whole TCP connection.
- Deadlines (`grpc-timeout`) are honored and mapped to Pulsate's request timeouts.

## Streaming

- Request and response bodies are **streamed by default** — never fully buffered unless a middleware explicitly opts in (e.g., a body transform or a signature check), and even then bounded by a configurable cap with a `413` past the limit.
- **Backpressure is end-to-end:** Pulsate only reads from the client as fast as the upstream accepts, and vice versa, by coupling the two body streams. A slow consumer slows the producer instead of growing buffers.
- Chunked transfer, content-length, and EOF framing are all handled uniformly behind the `Body` abstraction.

## Connection pooling & keep-alive

- **Downstream (client-facing):** keep-alive is on; per-connection limits cap requests-per-connection, idle time, and total lifetime so clients can't pin resources indefinitely.
- **Upstream (backend-facing):** `pulsate-proxy` maintains per-upstream connection pools (`pool { max_idle, idle_timeout, max_per_host }`). Connections are reused across requests; HTTP/2 upstreams multiplex many requests over few connections. Pools are health-aware (a connection to an ejected backend is not reused) and survive config reloads when the upstream is unchanged (see [02. Architecture](02-architecture.md#hot-reload-architecture)).
- **Happy-eyeballs / connection racing** to upstreams with multiple addresses reduces tail connect latency.

## Timeouts

Every phase has a bounded, configurable timeout — there is no unbounded wait anywhere on the request path:

| Timeout | Default | Applies to |
|---|---|---|
| `handshake` | 10s | TLS/QUIC negotiation |
| `header_read` | 10s | reading the request head ([Decode]) |
| `request` | 30s | total request-to-first-byte-of-response |
| `upstream.connect` | 2s | establishing/reusing an upstream conn |
| `upstream.response` | 30s | upstream time-to-first-byte |
| `idle` (keep-alive) | 60s | downstream idle between requests |
| `stream_idle` | 60s | no progress on a streaming body |
| `shutdown.grace` | 30s | drain on reload/stop |

Timeouts are layered (a slow-loris client trips `header_read`; a hung backend trips `upstream.response`) and each maps to a specific status + error code ([25. Error Catalog](25-error-and-status-catalog.md)).

## Buffer management

- **Per-worker buffer pools** (`pulsate-util`) recycle read/write buffers, so steady-state request handling is allocation-light. Pool sizing adapts to observed request sizes within configured caps.
- Bodies use **`Bytes`** (reference-counted, cheaply sliceable) so passing a body through middleware and into the proxy clones pointers, not bytes.
- Header maps reuse small-vector storage to avoid heap allocation for the common (few-headers) case.
- All buffers are **bounded**: header buffer caps, max body buffer, and pool high-water marks ensure memory is a function of configured limits, not input.

## Zero-copy opportunities

Pulsate pursues zero-copy where it is safe and measurable:
- **Static file serving** uses `sendfile`/`splice` on supported platforms to move file bytes to the socket without copying through user space (TLS paths fall back to userspace, since encryption requires touching the data — though **kTLS** can restore zero-copy for TLS where the kernel supports it; tracked as an optimization).
- **Proxy body relay** passes `Bytes` chunks through without copying; the same buffer that came off the upstream socket is written to the client socket.
- **Vectored I/O** (`writev`) sends headers + body chunks in one syscall.
- **kTLS / kernel offload** and io_uring (via a future `pulsate-rt` backend) are the next frontier for reducing copies and syscalls on the hottest paths (see [10. Performance](10-performance.md)).

These are opportunities, applied where profiling shows benefit — correctness and memory-safety are never traded for a copy elision.

## Cross-references
- [02. Architecture](02-architecture.md) — connection & request lifecycle, buffer/memory model.
- [06. Reverse Proxy](06-reverse-proxy.md) — upstream selection, pooling policy, retries.
- [09. Security](09-security.md) — TLS hardening, mTLS, protocol abuse defenses.
- [10. Performance](10-performance.md) — zero-copy, io_uring, kTLS, syscall reduction.
- [08. Cache](08-cache.md) — range requests and conditional requests over these protocols.
