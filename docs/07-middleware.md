# 07. Middleware

> The pipeline: how the `~>` flow becomes an ordered chain of middleware around a terminal handler, the Ingress/Egress/Recover phases and their execution order, the middleware contract, and how built-in, custom, and plugin middleware all share one model.

**Contents**
- [The pipeline model](#the-pipeline-model)
- [Execution order](#execution-order)
- [The middleware contract](#the-middleware-contract)
- [Request (Ingress) middleware](#request-ingress-middleware)
- [Response (Egress) middleware](#response-egress-middleware)
- [Error (Recover) middleware](#error-recover-middleware)
- [Short-circuiting & control flow](#short-circuiting--control-flow)
- [Built-in middleware](#built-in-middleware)
- [Custom & plugin middleware](#custom--plugin-middleware)
- [Composition, ordering rules & pitfalls](#composition-ordering-rules--pitfalls)
- [Cross-references](#cross-references)

---

## The pipeline model

A route compiles to a **pipeline**: an ordered list of middleware wrapping a single terminal handler. Pulsate uses an **onion model** — each middleware wraps the next, sees the request on the way in (Ingress) and the response on the way out (Egress) — but expressed in config as a flat, readable left-to-right chain:

```
route /api/* ~> cors ~> rate_limit(100/min) ~> jwt ~> proxy(@api)

        Ingress  →  cors → rate_limit → jwt → [handler: proxy] 
        Egress   ←  cors ← rate_limit ← jwt ← [handler response]
```

Conceptually this is the classic wrap-the-next-handler design, but Pulsate compiles the chain into a **flat driver with an index** rather than deeply nested closures. The driver walks the middleware list forward for Ingress, invokes the handler, then walks backward for Egress. This keeps stack depth constant regardless of chain length, makes the pipeline introspectable (the dashboard can show each step), and lets a middleware cleanly short-circuit by telling the driver to stop advancing.

## Execution order

The three phases map onto the [request lifecycle stages](02-architecture.md#request-lifecycle):

```
[5 Ingress]   middleware[0].on_request → middleware[1].on_request → … → middleware[n].on_request
[6 Dispatch]  handler.handle  (may run [7 Upstream] for proxy)
[8 Egress]    middleware[n].on_response → … → middleware[1].on_response → middleware[0].on_response
[Recover]     on any Error: nearest middleware.on_error chain → synthesized response → resume Egress
[10 Finalize] always runs once: access log, metrics, trace close
```

Rules:
1. **Ingress runs in written (left-to-right) order.** The first step you list is the first to see the request — so put gatekeepers (WAF, auth, rate limit) early.
2. **Egress runs in reverse order.** The outermost middleware (leftmost) is the last to touch the response — so wrapping concerns (compression, response headers, tracing spans) nest correctly: `cors` opened first closes last.
3. **Dispatch is exactly one handler.** A route has one terminal handler; reaching it is the boundary between Ingress and Egress.
4. **Finalize is guaranteed.** Even on panic or error, Finalize emits exactly one access-log line and closes the trace span/metrics for the request.

This reverse-on-the-way-out symmetry is why, e.g., a tracing middleware placed first measures the *entire* pipeline including all other middleware, and a compression middleware placed early compresses the final response after all other response edits are done.

## The middleware contract

All middleware implement one `pulsate-core` trait (illustrative signatures — design contract, not implementation):

```rust
// Design sketch only — no implementation here.
trait Middleware {
    // Ingress: inspect/modify request; decide whether to continue.
    async fn on_request(&self, ctx: &mut RequestCtx) -> Flow;

    // Egress: inspect/modify response on the way out (default: pass-through).
    async fn on_response(&self, ctx: &mut RequestCtx) -> Flow { Flow::Continue }

    // Recover: optionally handle an error raised downstream (default: propagate).
    async fn on_error(&self, ctx: &mut RequestCtx, err: &PulsateError) -> Recovery {
        Recovery::Propagate
    }
}

enum Flow { Continue, Stop }          // Stop = short-circuit: skip remaining Ingress + handler
enum Recovery { Propagate, Handled }  // Handled = a response was synthesized; resume Egress
```

- **`RequestCtx`** is the single argument: it exposes the request, the response-in-progress, the matched route + captures, the snapshot, request-scoped typed extensions (a `TypeMap` for passing data between middleware, e.g. the JWT claims), timing, and the request ID.
- Middleware are **stateless and shareable** (`Arc`-held, instantiated once per snapshot from config); any per-request state lives in `RequestCtx` extensions, never in the middleware object.
- A `Handler` is the terminal analog: `async fn handle(&self, ctx: &mut RequestCtx) -> Result<(), PulsateError>`.

This one contract is what makes built-in, custom, and plugin middleware interchangeable: the pipeline driver only knows the trait.

## Request (Ingress) middleware

Operate on the inbound request before Dispatch. Typical responsibilities and recommended ordering (earlier = first):

1. **Edge protections** — `waf`, connection/`rate_limit` (reject abusive traffic before spending work).
2. **Authn/authz** — `jwt`, `basic_auth`, `forward_auth`, `require(...)` (reject unauthorized before touching the backend).
3. **Request shaping** — `headers(...)`, `rewrite`, `strip_prefix`, normalization.
4. **Caching lookup** — `cache(@c)` checks for a fresh stored response and may short-circuit (serve from cache, skip the handler entirely).

An Ingress middleware may modify the request, attach data to `ctx` extensions, or **Stop** (short-circuit) by producing a response itself (e.g., 401, 429, or a cache hit).

## Response (Egress) middleware

Operate on the response after Dispatch, in reverse order:

- **`compress`** — negotiate and apply gzip/brotli/zstd to the response body.
- **`headers(...)`** — add/override response headers (security headers, caching headers).
- **`cors`** — attach the appropriate CORS response headers based on the request captured at Ingress.
- **`cache(@c)`** — store the fresh response for future requests (the same middleware that did the Ingress lookup does the Egress store).
- **tracing/metrics finalizers** — annotate spans with response status/size.

Because Egress is symmetric, a single middleware object commonly implements both `on_request` and `on_response` (e.g., `cache` looks up on the way in and stores on the way out).

## Error (Recover) middleware

When any stage returns a `PulsateError`, the driver unwinds to the **Recover** phase:

- It walks the already-entered middleware's `on_error` in reverse, giving each a chance to handle the error (`Recovery::Handled`) or let it propagate.
- If none handles it, a default mapper converts the error to a response per the [25. Error Catalog](25-error-and-status-catalog.md) (status, `application/problem+json` body, headers, request ID).
- Control then **resumes at Egress** so response middleware (security headers, CORS, compression, logging) still apply to the error response. This guarantees error responses are as well-formed and observable as success responses.
- Custom error pages and fallbacks are configured via an `on_error` directive (`route ... ~> on_error(respond(status=503, body=@maintenance_html))`).

## Short-circuiting & control flow

Short-circuiting is first-class and explicit:
- An Ingress middleware returning `Flow::Stop` (after writing a response into `ctx`) **skips the remaining Ingress middleware and the handler**, jumping straight to Egress for the middleware that already ran. Example: `rate_limit` over budget writes `429` and stops — `jwt` and `proxy` never run, but `compress`/`headers` on the way out still apply.
- A cache hit is the canonical short-circuit: `cache` serves the stored response at Ingress and stops, so the upstream is never contacted.
- Handlers like `redirect`/`respond` are terminal by nature.
- Short-circuits are **observable**: Finalize records which middleware terminated the request (surfaced in logs/metrics and the request inspector — [11. Dashboard](11-dashboard.md)).

## Built-in middleware

| Name | Phase(s) | Summary |
|---|---|---|
| `waf(@ruleset)` | Ingress | rule matching, geo/ASN/bot/IP policy |
| `rate_limit(rate, key=)` | Ingress | token-bucket/sliding-window limiting |
| `jwt(...)`, `basic_auth(...)`, `forward_auth(@svc)` | Ingress | authentication |
| `require(expr)` | Ingress | authorization predicate (claims/cert) |
| `cors(...)` | Ingress+Egress | preflight + response headers |
| `headers(set=, remove=)` | Ingress or Egress | header rewriting |
| `rewrite/strip_prefix` | Ingress | path manipulation |
| `cache(@c)` | Ingress+Egress | lookup + store |
| `compress(...)` | Egress | response compression |
| `timeout(d)` / `retry(...)` | wrap | per-route overrides |
| `on_error(handler)` | Recover | custom error handling |

Built-ins live in `pulsate-pipeline` (with `pulsate-waf`/`pulsate-cache` providing the heavy logic) and register their factories in the `Registry` ([02. Architecture](02-architecture.md#dependency-injection-strategy)).

## Custom & plugin middleware

Three ways to extend the pipeline, all implementing the same `Middleware` contract:

1. **Built-in (in-tree Rust)** — for core features and performance-critical paths; compiled into Pulsate.
2. **WASM plugin** — `plugin.<name>(...)` appears as a step like any other. The host (`pulsate-plugin`) adapts the WASM component's exported functions to the `Middleware` trait, runs it sandboxed with fuel/epoch limits and capability grants, and marshals a *restricted, stable view* of `RequestCtx` across the ABI ([12. Plugins](12-plugins.md)). This is the recommended path for third-party middleware: no recompiling Pulsate, language-agnostic, sandboxed.
3. **Native extension (embedders)** — projects building on the crates can implement `Middleware` directly for maximum performance, registering it in the `Registry`.

From the route's perspective these are indistinguishable: `route /* ~> waf ~> plugin.geoblock ~> my_native_mw ~> proxy(@api)`.

## Composition, ordering rules & pitfalls

- **Put gatekeepers first.** WAF/auth/rate-limit before expensive work; otherwise you pay to reject. `p8 validate` warns about obvious anti-patterns (e.g., `proxy` before `jwt`, or `compress` before a body-transforming step).
- **Cache placement matters.** `cache` should sit *outside* (left of) `proxy` so hits skip the upstream, but *inside* (right of) auth if responses are user-specific — Pulsate lets you express both and warns when caching authenticated responses without a `vary`/key that includes identity.
- **Compression vs. range/ETag.** `compress` interacts with caching and range requests; Pulsate coordinates these (compression-aware caching in [08. Cache](08-cache.md)) so you don't get corrupted partial responses.
- **One handler per route.** Two terminal handlers in a chain is a load-time error.
- **Idempotency for retries.** `retry` wrapping a non-idempotent route is flagged.

The pipeline's flat-driver design means none of these compositions risk stack overflow or hidden re-entrancy, and every step is visible to the request inspector for debugging.

## Cross-references
- [02. Architecture](02-architecture.md) — Ingress/Egress/Recover within the lifecycle; `RequestCtx`.
- [04. Configuration](04-configuration.md) — the `~>` syntax and built-in step list.
- [06. Reverse Proxy](06-reverse-proxy.md) — the `proxy` handler and rewriting.
- [08. Cache](08-cache.md) — the `cache` middleware's lookup/store semantics.
- [12. Plugins](12-plugins.md) — WASM middleware host model and the ABI.
- [25. Error Catalog](25-error-and-status-catalog.md) — error-to-response mapping in Recover.
