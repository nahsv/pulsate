# 12. Plugins

> The extension system: a sandboxed WebAssembly plugin model with a stable, versioned host interface, so anyone can add middleware, handlers, matchers, and integrations in any language — without forking or recompiling Pulsate.

**Contents**
- [Why WebAssembly](#why-webassembly)
- [Plugin architecture](#plugin-architecture)
- [Extension types](#extension-types)
- [The host interface (WIT)](#the-host-interface-wit)
- [Sandboxing & capabilities](#sandboxing--capabilities)
- [Lifecycle & performance](#lifecycle--performance)
- [Versioning & API stability](#versioning--api-stability)
- [Developer SDK](#developer-sdk)
- [Distribution & marketplace](#distribution--marketplace)
- [Cross-references](#cross-references)

---

## Why WebAssembly

The extension model is the make-or-break of a gateway's ecosystem. Pulsate rejects the common options and chooses WASM deliberately:

| Approach | Problem | 
|---|---|
| **Recompile (Go plugins/C modules)** | every plugin requires building a custom binary; breaks "one binary"; version lock-in |
| **Native dynamic loading (dlopen)** | no sandbox — a bad plugin crashes or compromises the whole proxy; unsafe ABI |
| **Embedded scripting (Lua)** | one language, limited ecosystem, performance ceiling, weak typing |
| **WebAssembly (Pulsate's choice)** | language-agnostic, **sandboxed** (memory + capability isolation), near-native speed, stable ABI, hot-loadable, distributable as a single artifact |

WASM gives us safety *and* extensibility without sacrificing the single-binary, secure-by-default model: a plugin is data the binary loads, runs in a sandbox, and can revoke — not code linked into the trusted core.

## Plugin architecture

`pulsate-plugin` hosts plugins using **Wasmtime** with the **WebAssembly Component Model** and **WASI**. A plugin is a `.wasm` component implementing one or more of Pulsate's extension *worlds* (WIT-defined interfaces).

```
pulsate.flow: plugins { load geoblock { source "geoblock.wasm"; config {...} } }
                          │ load at startup/reload
                          ▼
            ┌──────────── pulsate-plugin (host) ────────────┐
            │ Wasmtime engine (shared, AOT-compiled)       │
            │ per-plugin: linked instance pool             │
            │ host functions (WIT imports) ── capability-gated
            └───────────┬──────────────────────────────────┘
                        │ adapts to pulsate-core traits
                        ▼
   route /* ~> plugin.geoblock(allow=[US]) ~> proxy(@api)
              (a Middleware, indistinguishable from a built-in to the pipeline)
```

The host **adapts a plugin to the same `pulsate-core` traits** (`Middleware`, `Handler`, `Matcher`, …) the rest of Pulsate uses ([07. Middleware](07-middleware.md)). To the pipeline driver, `plugin.geoblock` is just another `Middleware`. Plugins register their factories in the `Registry` ([02. Architecture](02-architecture.md#dependency-injection-strategy)) when loaded.

## Extension types

A plugin can implement any of these worlds:
- **Middleware** — inspect/modify request/response, short-circuit (the most common: auth providers, transforms, custom headers, geo/bot logic).
- **Handler** — a terminal handler (custom origin, mock, special protocol bridge).
- **Matcher** — a custom route predicate (`route /* [plugin.match.feature_flag=on]`).
- **Auth provider** — a `forward_auth`/token-validation backend.
- **Cache/Secrets/Discovery backend** — implement `CacheStore`, `SecretsBackend`, or a service discoverer.
- **Observability sink** — a custom metrics/trace/log exporter.

One plugin may export several (e.g., a middleware + a matcher) sharing config.

## The host interface (WIT)

The boundary is defined in **WIT** (WASM Interface Types) so it is language-neutral and strongly typed. Illustrative WIT (design sketch, not final):

```wit
// pulsate:plugin world (versioned) — design sketch
package pulsate:plugin@1.0.0;

interface http {
  record request  { method: string, path: string, headers: list<tuple<string,string>>, ... }
  record response { status: u16, headers: list<tuple<string,string>>, ... }
  enum flow { continue, stop }
}

world middleware {
  use http.{request, response, flow};
  // host → guest
  export on-request: func(ctx: borrow<request-ctx>) -> flow;
  export on-response: func(ctx: borrow<request-ctx>) -> flow;
  // guest → host (capability-gated imports)
  import log: func(level: u8, msg: string);
  import kv-get: func(key: string) -> option<list<u8>>;   // only if granted
  import http-fetch: func(req: request) -> result<response>; // only if granted
}
```

The plugin sees a **restricted, stable view** of the request context across the ABI — not Pulsate's internal `RequestCtx` struct — which decouples the plugin contract from internal refactors and is the thing we promise stability on.

## Sandboxing & capabilities

A plugin is **untrusted by default** and confined:
- **Memory isolation:** WASM linear memory is fully isolated; a plugin cannot read Pulsate's memory or another plugin's. A misbehaving plugin can corrupt only itself.
- **Capability-based access:** a plugin can do *nothing* with the outside world unless granted. Grants are explicit in config:
  ```
  load mytransform {
    source "transform.wasm"
    capabilities { net ["api.internal:443"]; kv read; env ["FEATURE_X"]; fs none }
  }
  ```
  No ambient filesystem, network, env, clock, or randomness beyond WASI grants — and host imports (`http-fetch`, `kv-get`) are only linked if the capability is present.
- **Resource limits:** Wasmtime **fuel** and **epoch interruption** bound CPU per invocation (a plugin can't hang a request — it's killed past its budget); memory is capped; the request fails closed/open per policy if a plugin traps.
- **Determinism options:** plugins can be denied non-deterministic host calls for reproducibility.
- **Fail policy:** per-plugin `on_error fail_open|fail_closed` decides whether a trapping plugin blocks or passes the request. Security-critical plugins fail closed.

This makes third-party code a *bounded* risk — central to the [21. Threat Model](21-threat-model.md).

## Lifecycle & performance

- **Load:** at startup/reload, the host reads the `.wasm`, validates it, **AOT-compiles** it (Cranelift) once, and caches the compiled artifact; instances are cheap to create thereafter.
- **Instance pooling:** a pool of pre-instantiated, reset-able instances per plugin avoids per-request instantiation cost; the Component Model's instance reset gives clean state per request without re-init.
- **Hot reload:** plugins reload with config (new version swapped atomically; in-flight requests finish on the old instance).
- **Overhead:** near-native execution; the marshaling cost across the ABI is the main expense, kept low by passing borrowed views and avoiding copies. For latency-critical paths where even that is too much, a native in-tree extension is the escape hatch ([07. Middleware](07-middleware.md)).
- **Observability:** per-plugin metrics (invocations, duration, traps, fuel used) and logs are first-class ([26. Metrics Catalog](26-metrics-and-slo-catalog.md)).

## Versioning & API stability

- The **host ABI is versioned independently** of the Pulsate binary via the WIT *world version* (`pulsate:plugin@1.x`). A binary supports a **range** of ABI versions; a plugin declares the world it targets.
- **Stability guarantee:** within an ABI major, plugins keep working across Pulsate upgrades. Breaking the ABI is a major-version event with a deprecation window and a compatibility shim where feasible.
- **Capability/permission additions** are backward-compatible (new optional imports); removals are breaking and scheduled.
- This independent versioning (binary vs config `flow_version` vs plugin ABI — [03. Repository](03-repository.md#versioning)) means upgrading one rarely forces upgrading the others.

## Developer SDK

`pulsate-sdk` makes writing plugins pleasant:
- **Rust-first:** ergonomic wrappers over the raw WIT bindings — implement a `Middleware`-shaped trait, annotate with a macro, `cargo build --target wasm32-wasip2`, done. Hides the ABI boilerplate.
- **Other languages:** because the contract is WIT/Component Model, bindings can be generated for any language with WASM component support (Go via TinyGo, JS/TS via jco, Python, etc.); the SDK ships a starter template per supported language.
- **Local dev loop:** `pulsate plugin new <name>` scaffolds a project; `pulsate plugin test` runs it against a local harness with mocked requests; `pulsate plugin run` hot-loads it into a dev server ([13. CLI](13-cli.md)).
- **Docs & examples:** a cookbook of real plugins (geoblock, header transform, custom auth, A/B flagger) — see [17. Documentation](17-documentation.md).

## Distribution & marketplace

- **Artifacts:** a plugin is a single `.wasm` file, distributable from a path, a URL, or an **OCI registry** (`source "oci://registry/plugins/x:1.2.0"`) — versioned, signed, and pullable like a container image.
- **Signing & provenance:** plugins can be **signed (Sigstore/cosign)**; Pulsate can require signature verification before loading (`plugins { require_signed true; trusted_keys [...] }`) — a supply-chain control ([33. Release Engineering](33-release-engineering-and-supply-chain.md)).
- **Marketplace (future):** a curated registry of community/verified plugins with ratings, capability declarations shown up-front (so you see what a plugin asks for before installing), and one-line install. Capability transparency is the trust model ([20. Future](20-future.md)).

## Cross-references
- [02. Architecture](02-architecture.md) — traits, Registry, extension points.
- [07. Middleware](07-middleware.md) — how plugins plug into the pipeline.
- [21. Threat Model](21-threat-model.md) — plugin/supply-chain threats and the sandbox boundary.
- [33. Release Engineering & Supply Chain](33-release-engineering-and-supply-chain.md) — signing/provenance.
- [13. CLI](13-cli.md) — `pulsate plugin` developer commands.
