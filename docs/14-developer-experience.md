# 14. Developer Experience

> The first ten minutes and the daily loop: installation, project initialization, automatic application detection (Rails/Node/Go/Rust), Docker and Kubernetes integration, and the development/debug modes that make Pulsate a pleasure to build on.

**Contents**
- [The DX thesis](#the-dx-thesis)
- [Installation](#installation)
- [Project initialization](#project-initialization)
- [Automatic application detection](#automatic-application-detection)
- [Framework support](#framework-support)
- [Docker support](#docker-support)
- [Kubernetes support](#kubernetes-support)
- [Development mode](#development-mode)
- [Debug mode](#debug-mode)
- [Cross-references](#cross-references)

---

## The DX thesis

Pulsate measures success as **time-to-correct-configuration**. The bar: a developer with an app and a domain serves it over valid HTTPS in under a minute, without reading docs. Every feature below exists to shorten that path and to make the inevitable debugging fast. DX is not polish applied late — it is an acceptance criterion in [19. Milestones](19-milestones.md).

## Installation

One step, many channels, no dependencies (one static binary):

```bash
# Universal installer (detects OS/arch, verifies signature)
curl -fsSL https://squaretick.dev/pulsate/install.sh | sh

# Package managers
brew install p8                      # macOS / Linuxbrew
apt install p8  /  dnf install p8 # Debian/RHEL (signed repos)
scoop install p8                     # Windows
cargo binstall p8                    # Rust users

# Container
docker run -p 80:80 -p 443:443 ghcr.io/p8/p8:latest

# Direct download (static binary + checksum + signature + SBOM)
```

Every artifact is signed (Sigstore/cosign) and ships an SBOM; the installer verifies before placing the binary ([33. Release Engineering](33-release-engineering-and-supply-chain.md)). No OpenSSL, no runtime, nothing else to install.

## Project initialization

`p8 init` bootstraps a config by **looking at your project**, not by asking twenty questions:

```bash
$ cd my-app && p8 init
Detected: Rails app (Procfile, config.ru) on port 3000
Detected: Postgres (skipped — not a web service)
Domain? [my-app.localhost] app.example.com

Wrote pulsate.flow:
  site app.example.com {
    tls auto
    route /* ~> proxy(http://localhost:3000)
  }
Run it:  p8 up --watch
```

- Generates a minimal, idiomatic `pulsate.flow` tuned to what it found.
- `p8 init --template <api|spa|fullstack|microservices>` for common shapes.
- Idempotent and safe: never overwrites an existing config without `--force` (and shows a diff).

## Automatic application detection

`pulsate-cli` includes a **detector** that infers your stack from filesystem signals and running processes, so `init` and dev mode "just work":

| Signal | Inference |
|---|---|
| `Gemfile` + `config.ru`/`Procfile` | Rails/Rack → default port 3000 |
| `package.json` (scripts, framework deps) | Node → Next/Vite/Express, dev port (3000/5173/8080) |
| `go.mod` + `main.go` | Go service → detect `http.ListenAndServe` port |
| `Cargo.toml` (axum/actix/etc.) | Rust service → detect bound port |
| `manage.py` / `pyproject` | Django/FastAPI → 8000 |
| `Dockerfile` / `compose.yaml` | containerized → map exposed ports |
| `index.html` + build dir | static/SPA → serve files with SPA fallback |
| running process on a local port | offer to proxy it |

Detection is **advisory and transparent** — it explains what it found and what it assumed; you can always override. It never executes your app or runs untrusted code; it only reads manifests and (optionally) lists listening ports.

## Framework support

First-class recipes (documented in [17. Documentation](17-documentation.md), encoded as `init` templates):

- **Rails / Rack:** proxy to Puma; serve `public/` assets directly through Pulsate (with caching/compression) so Ruby never serves static files; ActionCable WebSocket route; `X-Sendfile`-style offload.
- **Node:** Next.js/Remix/Express/Nest — proxy to the app, serve `_next/static`/build assets via Pulsate cache, WebSocket/SSE passthrough, HMR-friendly in dev.
- **Go:** proxy to the service; gRPC and gRPC-Web handlers if a `.proto`/grpc dependency is detected; health-check wiring.
- **Rust:** axum/actix/etc. — proxy, plus the option to embed Pulsate as a library for single-process deployments (the crate split makes this clean — [03. Repository](03-repository.md)).
- **Static/SPA:** `files()` with SPA fallback, immutable-asset caching, and pre-compression.

Each recipe yields a few lines of Flow and turns on the right batteries (cache for assets, compression, security headers) by default.

## Docker support

- **Slim, distroless images** (`ghcr.io/p8/p8`), multi-arch (amd64/arm64), tiny because it's one static binary.
- **Compose-native:** Pulsate detects sibling services in `compose.yaml` and can generate routes for them (service name → upstream). A typical edge service:
  ```yaml
  services:
    p8:
      image: ghcr.io/p8/p8
      ports: ["80:80", "443:443"]
      volumes: ["./pulsate.flow:/etc/p8/pulsate.flow:ro"]
    api: { build: ./api }     # auto-discoverable as upstream http://api:8080
  ```
- **Labels (optional):** for teams who like Traefik-style labels, Pulsate can read container labels as a *config source* — but the file remains canonical (no label sprawl required). See [16. Deployment](16-deployment.md).
- Reads secrets from Docker/Compose secrets via the secrets backend.

## Kubernetes support

Pulsate is a credible ingress/gateway without a separate control plane:
- **Gateway API (preferred):** implements the Kubernetes **Gateway API** (`GatewayClass`/`Gateway`/`HTTPRoute`/`GRPCRoute`/`TLSRoute`), the modern standard — so Pulsate is a drop-in gateway controller.
- **Native CRD:** a `PulsateConfig`/`PulsateRoute` CRD exposes Flow's full power (WAF, cache, plugins) for features beyond Gateway API's surface, watched by the control plane as a [config source](02-architecture.md#configuration-loading).
- **Ingress (legacy):** classic `Ingress` resources supported for migration.
- **Service discovery:** native EndpointSlice watching for dynamic upstreams ([06. Reverse Proxy](06-reverse-proxy.md)); no kube-proxy round-trip needed.
- **Operations:** Prometheus metrics, OTLP traces, readiness/liveness probes, leader election for shared cert issuance ([16. Deployment](16-deployment.md)), and a Helm chart / operator. Certs and rate-limit/cache state shared across pods via the cluster/Redis backends.

## Development mode

`p8 up --watch` (or `p8 dev`) optimizes for the inner loop:
- **Hot config reload** on save (sub-second, zero dropped connections).
- **Local HTTPS that works:** automatically provisions a locally-trusted certificate (an internal dev CA installed into the system/browser trust store, like mkcert) for `*.localhost` and custom dev domains — real HTTPS in dev without ACME or warnings.
- **Helpful errors in the browser:** in dev, upstream-down/5xx responses show a friendly diagnostic page (which route matched, why it failed) instead of a bare 502.
- **Live request inspector** in the terminal (`p8 inspect`) and dashboard, on by default in dev.
- **Auto-proxy:** detects your app's dev server and proxies it; survives the app restarting (connection ret/breaker tuned lenient in dev).

## Debug mode

For diagnosing the gateway itself:
- `--debug` raises log verbosity, annotates responses with `X-Pulsate-*` debug headers (matched route, cache result, upstream, timings) — gated to dev/trusted networks.
- `p8 doctor` checks the environment (fds, ports, DNS, kernel features, permissions) and suggests fixes ([13. CLI](13-cli.md)).
- `p8 config explain <host> <path>` answers "why did this request go there?" deterministically.
- Profiling endpoints (`/v1/debug/pprof`, loopback) and `tokio-console` support for runtime stalls ([10. Performance](10-performance.md)).
- Verbose ACME/TLS tracing to debug certificate issues, with the staging-CA shortcut to avoid rate limits.

## Cross-references
- [13. CLI](13-cli.md) — `init`, `up --watch`, `inspect`, `doctor`, `dev`.
- [04. Configuration](04-configuration.md) — the Flow configs `init` generates.
- [16. Deployment](16-deployment.md) — Docker/Compose/K8s/systemd in production.
- [02. Architecture](02-architecture.md) — hot reload and the dev-mode local CA.
- [17. Documentation](17-documentation.md) — framework tutorials and recipes.
