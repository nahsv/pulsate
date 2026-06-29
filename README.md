# Pulsate

[![crates.io](https://img.shields.io/crates/v/pulsate.svg)](https://crates.io/crates/pulsate)
[![docs.rs](https://img.shields.io/docsrs/pulsate)](https://docs.rs/pulsate)
[![downloads](https://img.shields.io/crates/d/pulsate.svg)](https://crates.io/crates/pulsate)
[![CI](https://github.com/squaretick/pulsate/actions/workflows/ci.yml/badge.svg)](https://github.com/squaretick/pulsate/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/rustc-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](#license)

A reverse-proxy gateway written in Rust. One static binary, one config file, one
command — automatic TLS, caching, a WAF, observability, an admin API, and a WASM
plugin host, without bolting on a second tool.

The reverse-proxy world splits into developer-friendly tools you outgrow and
infrastructure-grade tools that need a control plane and a team to run. Pulsate is
an attempt to refuse that trade-off: the same config that starts a single box
scales, unchanged in shape, to a multi-site deployment.

> Status: under active development. The data plane, configuration language,
> reverse proxy, caching, WAF, observability, admin API, and plugin host are
> implemented and tested. See [Status](#status) for what isn't wired up yet.

## Install

Every channel installs the **`pulsate`** binary.

```sh
# Shell installer (Linux/macOS) — downloads the prebuilt release binary
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/squaretick/pulsate/main/scripts/install.sh | sh

# Cargo (any platform with Rust)
cargo install pulsate
cargo binstall pulsate           # prebuilt binary, no compile

# Homebrew (macOS/Linux)
brew install squaretick/tap/pulsate

# Debian / Ubuntu
curl -fsSLO https://github.com/squaretick/pulsate/releases/latest/download/pulsate_amd64.deb
sudo apt install ./pulsate_amd64.deb

# Fedora / RHEL
sudo dnf install https://github.com/squaretick/pulsate/releases/latest/download/pulsate.x86_64.rpm

# Docker
docker run --rm -p 8080:8080 ghcr.io/squaretick/pulsate:latest
```

Or build from source (Rust 1.86+):

```sh
cargo build --release
```

Prebuilt binaries, `.deb`/`.rpm` packages, and the container image are produced
for every tagged release by [`.github/workflows/release.yml`](.github/workflows/release.yml).
The Debian package installs a systemd unit; enable it with `sudo systemctl enable --now pulsate`.

Releases — version bumps, changelogs, and crates.io publishing across all crates —
are automated with [release-plz](https://release-plz.dev); see [RELEASING.md](RELEASING.md).

## Quick start

```sh
# Validate a config, then serve it.
pulsate validate examples/spa-api.flow
pulsate up examples/static.flow --listen 127.0.0.1:8080
```

A `pulsate.flow` reads the way a request flows — match, then a pipeline, then a
handler:

```flow
upstream api {
  target http://127.0.0.1:8080
  policy least_conn
}

site app.example.com {
  tls auto
  route /api/* ~> strip_prefix("/api")
              ~> cors(origins=["https://app.example.com"])
              ~> rate_limit(600/min, key=ip)
              ~> proxy(@api)
  route /*     ~> files("/srv/app")
}
```

## What it does

- **Routing & proxy** — host/path/method matching with deterministic precedence
  (exact > longest-prefix > catch-all); load balancing (round-robin, least-conn,
  random, ip-hash), retries, passive ejection / circuit breaking, and
  `X-Forwarded-*` / `Via` headers.
- **TLS** — rustls termination with SNI certificate selection and ALPN
  (`h2`, `http/1.1`); HTTP/2 auto-negotiation.
- **Caching** — in-memory store with RFC-9111 freshness, `Vary` keys,
  stale-while-revalidate, and tag-based purge.
- **Security** — a signature WAF (block/detect), fixed-window rate limiting with
  `RateLimit-*` headers, CIDR IP allow/deny, and a hash-chained audit log.
- **Observability** — Prometheus metrics, ULID request IDs, and structured JSON
  access logs.
- **Operations** — a loopback REST admin API with bearer-token auth and RBAC,
  plus an embedded dashboard.
- **Extensibility** — a capability-sandboxed WASM plugin host (Wasmtime) with a
  fuel budget and ABI versioning.
- **Migration** — `pulsate import nginx|caddy|haproxy|apache` translates an existing
  config to Flow and reports the fidelity of every mapping.

## Configuration

Configuration is a purpose-built language, [Flow](docs/04-configuration.md), not
YAML or a templated `nginx.conf`. It is declarative and typed — durations
(`30s`), sizes (`10MB`), rates (`100/min`), and `@references` are first-class, and
errors point at the line and column. TLS is automatic unless you opt out; the
admin API is loopback-only until you say otherwise.

```sh
pulsate validate pulsate.flow      # parse + validate, with diagnostics
pulsate config dump pulsate.flow   # print the typed config model
```

## CLI

One binary, a handful of subcommands. Run `pulsate <command> --help` for the full
flag list.

| Command | What it does |
| --- | --- |
| `pulsate up <config>` | Build the runtime from a config and serve it. |
| `pulsate validate <config>` | Parse and type-check a config without starting; non-zero exit on error. |
| `pulsate config dump <config>` | Validate, then print the resolved typed config model. |
| `pulsate import <fmt> <file>` | Translate an `nginx` / `caddy` / `haproxy` / `apache` config to Flow + fidelity report. |
| `pulsate plugin run <file> [input]` | Load a `.wasm`/`.wat` plugin and call its `eval(input)`. |
| `pulsate info` | Print version and the common-command summary (default when run with no args). |

Common `pulsate up` flags (defaults shown):

| Flag | Default | Purpose |
| --- | --- | --- |
| `--listen <addr>` | `127.0.0.1:8080` | Plain-HTTP listen address. |
| `--tls-listen <addr>` | — | TLS listen address; requires `--cert` and `--key`. |
| `--cert <path>` / `--key <path>` | — | PEM certificate chain and private key for the TLS listener. |
| `--metrics <addr\|off>` | `127.0.0.1:9100` | Prometheus metrics endpoint. |
| `--admin <addr\|off>` | `127.0.0.1:9180` | Admin API + dashboard endpoint (loopback by default). |
| `--admin-token <token>` | generated | Bearer token for the admin API; printed on startup if omitted. |
| `--http3-port <port>` | — | Advertise HTTP/3 via `Alt-Svc` on this UDP port. |

```sh
# Serve with TLS + metrics off, on all interfaces
pulsate up pulsate.flow \
  --listen 0.0.0.0:443 --tls-listen 0.0.0.0:443 \
  --cert fullchain.pem --key privkey.pem \
  --metrics off

# Convert an existing nginx config and inspect the result
pulsate import nginx /etc/nginx/nginx.conf > pulsate.flow
pulsate validate pulsate.flow
```

## Architecture

Pulsate is a single process split into a **data plane** that moves bytes and a
**control plane** that decides policy, joined by an immutable `ConfigSnapshot`
the data plane reads lock-free via `arc-swap`. A reload builds a new snapshot and
swaps a pointer; in-flight requests finish against the snapshot they started on.

The code is a Cargo workspace of 27 `pulsate-*` crates arranged so the data plane
never depends on the control plane — a rule enforced in CI by
`cargo xtask lint-layering`. See [docs/02-architecture.md](docs/02-architecture.md)
and [docs/03-repository.md](docs/03-repository.md).

## Crates

Every crate is published to crates.io and versioned in lockstep. Click a badge for
the crate, or the **docs** link for its API.

**Binary & CLI**

| Crate | Docs | Downloads | Description |
| --- | --- | --- | --- |
| [![pulsate](https://img.shields.io/crates/v/pulsate?label=pulsate)](https://crates.io/crates/pulsate) | [docs](https://docs.rs/pulsate) | [![downloads](https://img.shields.io/crates/d/pulsate.svg)](https://crates.io/crates/pulsate) | The `pulsate` binary: one gateway with TLS, caching, WAF, observability, admin API, and WASM plugins. |
| [![pulsate-cli](https://img.shields.io/crates/v/pulsate-cli?label=pulsate-cli)](https://crates.io/crates/pulsate-cli) | [docs](https://docs.rs/pulsate-cli) | [![downloads](https://img.shields.io/crates/d/pulsate-cli.svg)](https://crates.io/crates/pulsate-cli) | Implementations of the `pulsate` subcommands, kept separate so the binary stays thin and testable. |
| [![pulsate-migrate](https://img.shields.io/crates/v/pulsate-migrate?label=pulsate-migrate)](https://crates.io/crates/pulsate-migrate) | [docs](https://docs.rs/pulsate-migrate) | [![downloads](https://img.shields.io/crates/d/pulsate-migrate.svg)](https://crates.io/crates/pulsate-migrate) | Config importers: nginx / Caddy / HAProxy / Apache → Flow, with a fidelity report. |

**Foundation**

| Crate | Docs | Downloads | Description |
| --- | --- | --- | --- |
| [![pulsate-core](https://img.shields.io/crates/v/pulsate-core?label=pulsate-core)](https://crates.io/crates/pulsate-core) | [docs](https://docs.rs/pulsate-core) | [![downloads](https://img.shields.io/crates/d/pulsate-core.svg)](https://crates.io/crates/pulsate-core) | Shared vocabulary: `RequestCtx`, `Request`/`Response`, the `PulsateError` taxonomy, core traits, `ConfigSnapshot`. |
| [![pulsate-rt](https://img.shields.io/crates/v/pulsate-rt?label=pulsate-rt)](https://crates.io/crates/pulsate-rt) | [docs](https://docs.rs/pulsate-rt) | [![downloads](https://img.shields.io/crates/d/pulsate-rt.svg)](https://crates.io/crates/pulsate-rt) | Async runtime abstraction (Tokio backend); the seam for a future thread-per-core io_uring runtime. |
| [![pulsate-util](https://img.shields.io/crates/v/pulsate-util?label=pulsate-util)](https://crates.io/crates/pulsate-util) | [docs](https://docs.rs/pulsate-util) | [![downloads](https://img.shields.io/crates/d/pulsate-util.svg)](https://crates.io/crates/pulsate-util) | Buffer pools, duration/size parsing, and small shared helpers. |
| [![pulsate-config](https://img.shields.io/crates/v/pulsate-config?label=pulsate-config)](https://crates.io/crates/pulsate-config) | [docs](https://docs.rs/pulsate-config) | [![downloads](https://img.shields.io/crates/d/pulsate-config.svg)](https://crates.io/crates/pulsate-config) | Typed config model, snapshot build, validation, and the arc-swap-published config store. |
| [![pulsate-flow](https://img.shields.io/crates/v/pulsate-flow?label=pulsate-flow)](https://crates.io/crates/pulsate-flow) | [docs](https://docs.rs/pulsate-flow) | [![downloads](https://img.shields.io/crates/d/pulsate-flow.svg)](https://crates.io/crates/pulsate-flow) | The Flow language: hand-written lexer, recursive-descent parser, typed AST, span-accurate diagnostics. |

**Data plane**

| Crate | Docs | Downloads | Description |
| --- | --- | --- | --- |
| [![pulsate-net](https://img.shields.io/crates/v/pulsate-net?label=pulsate-net)](https://crates.io/crates/pulsate-net) | [docs](https://docs.rs/pulsate-net) | [![downloads](https://img.shields.io/crates/d/pulsate-net.svg)](https://crates.io/crates/pulsate-net) | Listeners, socket options (SO_REUSEPORT), connection limits, accept loop, graceful drain. |
| [![pulsate-tls](https://img.shields.io/crates/v/pulsate-tls?label=pulsate-tls)](https://crates.io/crates/pulsate-tls) | [docs](https://docs.rs/pulsate-tls) | [![downloads](https://img.shields.io/crates/d/pulsate-tls.svg)](https://crates.io/crates/pulsate-tls) | rustls server config: SNI certificate resolution, ALPN, manual certs (mTLS to follow). |
| [![pulsate-http](https://img.shields.io/crates/v/pulsate-http?label=pulsate-http)](https://crates.io/crates/pulsate-http) | [docs](https://docs.rs/pulsate-http) | [![downloads](https://img.shields.io/crates/d/pulsate-http.svg)](https://crates.io/crates/pulsate-http) | HTTP/1.1 + HTTP/2 serving over hyper: normalize to `Request`/`Response`, run terminal handlers. |
| [![pulsate-http3](https://img.shields.io/crates/v/pulsate-http3?label=pulsate-http3)](https://crates.io/crates/pulsate-http3) | [docs](https://docs.rs/pulsate-http3) | [![downloads](https://img.shields.io/crates/d/pulsate-http3.svg)](https://crates.io/crates/pulsate-http3) | HTTP/3 over QUIC. |
| [![pulsate-router](https://img.shields.io/crates/v/pulsate-router?label=pulsate-router)](https://crates.io/crates/pulsate-router) | [docs](https://docs.rs/pulsate-router) | [![downloads](https://img.shields.io/crates/d/pulsate-router.svg)](https://crates.io/crates/pulsate-router) | Routing table + matchers (host/path/regex/method/weighted). |
| [![pulsate-pipeline](https://img.shields.io/crates/v/pulsate-pipeline?label=pulsate-pipeline)](https://crates.io/crates/pulsate-pipeline) | [docs](https://docs.rs/pulsate-pipeline) | [![downloads](https://img.shields.io/crates/d/pulsate-pipeline.svg)](https://crates.io/crates/pulsate-pipeline) | Middleware engine (Ingress/Egress) and built-ins: strip_prefix, headers, cors. |
| [![pulsate-proxy](https://img.shields.io/crates/v/pulsate-proxy?label=pulsate-proxy)](https://crates.io/crates/pulsate-proxy) | [docs](https://docs.rs/pulsate-proxy) | [![downloads](https://img.shields.io/crates/d/pulsate-proxy.svg)](https://crates.io/crates/pulsate-proxy) | Reverse proxy: upstream pools, load balancing, retries, passive ejection / circuit breaking, forwarded headers. |
| [![pulsate-cache](https://img.shields.io/crates/v/pulsate-cache?label=pulsate-cache)](https://crates.io/crates/pulsate-cache) | [docs](https://docs.rs/pulsate-cache) | [![downloads](https://img.shields.io/crates/d/pulsate-cache.svg)](https://crates.io/crates/pulsate-cache) | HTTP caching: in-memory store, RFC-9111 freshness, validators, stale-while-revalidate, tag-based purge. |
| [![pulsate-waf](https://img.shields.io/crates/v/pulsate-waf?label=pulsate-waf)](https://crates.io/crates/pulsate-waf) | [docs](https://docs.rs/pulsate-waf) | [![downloads](https://img.shields.io/crates/d/pulsate-waf.svg)](https://crates.io/crates/pulsate-waf) | WAF signatures, rate limiting, IP allow/deny (CIDR), and a tamper-evident audit log. |

**Control plane**

| Crate | Docs | Downloads | Description |
| --- | --- | --- | --- |
| [![pulsate-control](https://img.shields.io/crates/v/pulsate-control?label=pulsate-control)](https://crates.io/crates/pulsate-control) | [docs](https://docs.rs/pulsate-control) | [![downloads](https://img.shields.io/crates/d/pulsate-control.svg)](https://crates.io/crates/pulsate-control) | Control-plane orchestrator and REST admin API (token auth + RBAC, loopback by default). |
| [![pulsate-acme](https://img.shields.io/crates/v/pulsate-acme?label=pulsate-acme)](https://crates.io/crates/pulsate-acme) | [docs](https://docs.rs/pulsate-acme) | [![downloads](https://img.shields.io/crates/d/pulsate-acme.svg)](https://crates.io/crates/pulsate-acme) | ACME plumbing: HTTP-01 challenge store, dynamic certificate store, on-demand allow-list. |
| [![pulsate-secrets](https://img.shields.io/crates/v/pulsate-secrets?label=pulsate-secrets)](https://crates.io/crates/pulsate-secrets) | [docs](https://docs.rs/pulsate-secrets) | [![downloads](https://img.shields.io/crates/d/pulsate-secrets.svg)](https://crates.io/crates/pulsate-secrets) | Secrets backends: environment and file (Vault / cloud KMS to follow). |
| [![pulsate-cluster](https://img.shields.io/crates/v/pulsate-cluster?label=pulsate-cluster)](https://crates.io/crates/pulsate-cluster) | [docs](https://docs.rs/pulsate-cluster) | [![downloads](https://img.shields.io/crates/d/pulsate-cluster.svg)](https://crates.io/crates/pulsate-cluster) | Clustering: membership, leader/peer roles, shared state. |
| [![pulsate-k8s](https://img.shields.io/crates/v/pulsate-k8s?label=pulsate-k8s)](https://crates.io/crates/pulsate-k8s) | [docs](https://docs.rs/pulsate-k8s) | [![downloads](https://img.shields.io/crates/d/pulsate-k8s.svg)](https://crates.io/crates/pulsate-k8s) | Kubernetes Gateway API controller: reconciles Gateway/HTTPRoute into a live config snapshot. |
| [![pulsate-observe](https://img.shields.io/crates/v/pulsate-observe?label=pulsate-observe)](https://crates.io/crates/pulsate-observe) | [docs](https://docs.rs/pulsate-observe) | [![downloads](https://img.shields.io/crates/d/pulsate-observe.svg)](https://crates.io/crates/pulsate-observe) | Observability: Prometheus metrics + exposition, request IDs, structured JSON access logs. |

**Plugins**

| Crate | Docs | Downloads | Description |
| --- | --- | --- | --- |
| [![pulsate-plugin](https://img.shields.io/crates/v/pulsate-plugin?label=pulsate-plugin)](https://crates.io/crates/pulsate-plugin) | [docs](https://docs.rs/pulsate-plugin) | [![downloads](https://img.shields.io/crates/d/pulsate-plugin.svg)](https://crates.io/crates/pulsate-plugin) | WASM plugin host (Wasmtime): fuel/epoch limits, capability sandbox, ABI versioning. |
| [![pulsate-sdk](https://img.shields.io/crates/v/pulsate-sdk?label=pulsate-sdk)](https://crates.io/crates/pulsate-sdk) | [docs](https://docs.rs/pulsate-sdk) | [![downloads](https://img.shields.io/crates/d/pulsate-sdk.svg)](https://crates.io/crates/pulsate-sdk) | Guest-side SDK for plugin authors. |

**UI & testing**

| Crate | Docs | Downloads | Description |
| --- | --- | --- | --- |
| [![pulsate-dashboard](https://img.shields.io/crates/v/pulsate-dashboard?label=pulsate-dashboard)](https://crates.io/crates/pulsate-dashboard) | [docs](https://docs.rs/pulsate-dashboard) | [![downloads](https://img.shields.io/crates/d/pulsate-dashboard.svg)](https://crates.io/crates/pulsate-dashboard) | The embedded dashboard: static assets served by the admin server. |
| [![pulsate-test](https://img.shields.io/crates/v/pulsate-test?label=pulsate-test)](https://crates.io/crates/pulsate-test) | [docs](https://docs.rs/pulsate-test) | [![downloads](https://img.shields.io/crates/d/pulsate-test.svg)](https://crates.io/crates/pulsate-test) | Test harness, fakes, and conformance utilities. |

## Building & testing

```sh
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo xtask lint-layering          # data plane ⇏ control plane
cargo deny check                   # licenses, advisories, bans
```

With [mise](https://mise.jdx.dev), `mise run check` runs the full gate.

## Documentation

The [`docs/`](docs/) directory holds the design: vision, architecture, the
configuration language, the reverse-proxy and middleware models, security, the
admin API, the error catalog, and more. Kubernetes manifests live in
[`deploy/kubernetes/`](deploy/kubernetes/) and runnable configs in
[`examples/`](examples/).

## Status

Implemented and tested: the HTTP/1.1 + HTTP/2 data plane, the Flow language and
snapshot/reload machinery, the reverse proxy, the middleware pipeline, caching,
the WAF and rate limiting, observability, the admin API and dashboard, the WASM
plugin host, and the nginx/Caddy/HAProxy/Apache importers.

Not yet wired up: live ACME certificate issuance (the challenge and certificate
stores exist; the protocol client does not), the QUIC/HTTP-3 transport (only
`Alt-Svc` discovery is emitted), the gRPC admin surface and event streams, the
cluster gossip transport, and the Kubernetes Gateway API controller.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or
<http://www.apache.org/licenses/LICENSE-2.0>). See
[docs/18-open-source.md](docs/18-open-source.md).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
licensed as above, without any additional terms or conditions.
