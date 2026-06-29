# Pulsate

[![crates.io](https://img.shields.io/crates/v/pulsate.svg)](https://crates.io/crates/pulsate)
[![docs.rs](https://img.shields.io/docsrs/pulsate)](https://docs.rs/pulsate)
[![CI](https://github.com/squaretick/pulsate/actions/workflows/ci.yml/badge.svg)](https://github.com/squaretick/pulsate/actions/workflows/ci.yml)
[![Rust](https://github.com/h4x0r/stem-branch/actions/workflows/rust.yml/badge.svg)](https://github.com/h4x0r/stem-branch/actions/workflows/rust.yml)
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

Every channel installs two identical binaries: **`pulsate`** and its short alias
**`pulsate`**.

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
# Validate a config, then serve it. `pulsate` is a shorter alias for `pulsate`.
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

## Architecture

Pulsate is a single process split into a **data plane** that moves bytes and a
**control plane** that decides policy, joined by an immutable `ConfigSnapshot`
the data plane reads lock-free via `arc-swap`. A reload builds a new snapshot and
swaps a pointer; in-flight requests finish against the snapshot they started on.

The code is a Cargo workspace of ~26 `pulsate-*` crates arranged so the data plane
never depends on the control plane — a rule enforced in CI by
`cargo xtask lint-layering`. See [docs/02-architecture.md](docs/02-architecture.md)
and [docs/03-repository.md](docs/03-repository.md).

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
