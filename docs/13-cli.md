# 13. CLI

> The single command surface: every `pulsate` subcommand, its flags, and what it does — from `pulsate up` to validation, diagnostics, benchmarking, migration, and upgrades. One binary, one CLI, discoverable and scriptable.

**Contents**
- [Design principles](#design-principles)
- [Global flags](#global-flags)
- [Lifecycle commands](#lifecycle-commands)
- [Configuration commands](#configuration-commands)
- [Certificate commands](#certificate-commands)
- [Cache commands](#cache-commands)
- [Diagnostics & inspection](#diagnostics--inspection)
- [Benchmark commands](#benchmark-commands)
- [Plugin commands](#plugin-commands)
- [Migration & import](#migration--import)
- [Upgrade & self-management](#upgrade--self-management)
- [Exit codes & scripting](#exit-codes--scripting)
- [Cross-references](#cross-references)

---

## Design principles

- **One binary, verb-first subcommands.** `pulsate <verb> [args] [--flags]`, built with `clap` (`pulsate-cli`). Consistent noun/verb structure (`pulsate cert list`, `pulsate cache purge`).
- **The CLI talks to the Admin API** for anything touching a running instance (`reload`, `cache purge`, `cert renew`), so the CLI and dashboard are the same control surface ([22. Admin API](22-admin-api.md)). Offline commands (`validate`, `fmt`, `import`) work without a running server.
- **Helpful by default:** `--help` everywhere with examples; `pulsate` with no args prints a friendly getting-started; the CLI *explains what it will do* for mutating actions and supports `--dry-run`.
- **Human and machine output:** default rich/colored output; `--output json|yaml` for scripting; quiet/verbose levels.

## Global flags

| Flag | Purpose |
|---|---|
| `-c, --config <path>` | config file (default `./pulsate.flow`, then `/etc/pulsate/pulsate.flow`) |
| `--admin <addr>` | admin API endpoint (default from config / `127.0.0.1:9180`) |
| `-o, --output <fmt>` | `text` (default), `json`, `yaml` |
| `-v/-vv`, `-q` | verbosity / quiet |
| `--no-color`, `--color <when>` | color control |
| `--profile <name>` | named connection profile (multi-instance / multi-env) |
| `--version`, `--help` | version / help |

## Lifecycle commands

| Command | Description |
|---|---|
| `pulsate up [--watch] [--detach]` | Validate config and start serving. `--watch` hot-reloads on file change; `--detach` runs in background. The **one command** to go from zero to a TLS gateway. |
| `pulsate run` | Foreground run intended for supervisors (systemd/Docker) — no daemonization, logs to stdout. |
| `pulsate reload` | Trigger a zero-downtime config reload of the running instance (validate → diff → atomic swap). |
| `pulsate down [--grace <dur>]` | Graceful shutdown with drain ([02. Architecture](02-architecture.md#graceful-shutdown)). |
| `pulsate status` | Show running state: uptime, listeners, active connections, snapshot hash, worker health, readiness. |
| `pulsate reload --rollback` | Roll back to the previous config snapshot. |

## Configuration commands

| Command | Description |
|---|---|
| `pulsate validate [file]` | Full validation (syntax, types, refs, invariants); precise `PLS-CFG-*` diagnostics; exit non-zero on error. CI-friendly. |
| `pulsate fmt [file] [--check]` | Canonical formatter for `.flow` (like `gofmt`); `--check` fails if unformatted. |
| `pulsate config dump [--effective]` | Print the resolved config (`--effective` = after includes/env/secrets, secrets redacted) — what Pulsate actually runs. |
| `pulsate config diff <a> <b>` | Diff two configs (or current vs file) at the snapshot level — shows the real behavioral change. |
| `pulsate config explain <host> <path> [--method GET]` | "What would happen to this request?" — prints the matched site/route, the middleware chain, and the handler. Routing as a debuggable function. |
| `pulsate config edit` | Open `$EDITOR`, validate on save, apply with confirmation. |

## Certificate commands

| Command | Description |
|---|---|
| `pulsate cert list` | Inventory: hosts, issuer, expiry, source, renewal status. |
| `pulsate cert renew [host] [--force]` | Trigger renewal (all or one host). |
| `pulsate cert show <host>` | Full chain, validity, OCSP status, fingerprints. |
| `pulsate cert import --cert <f> --key <f> --hosts <...>` | Install a manual certificate. |
| `pulsate cert challenge-status` | Recent ACME challenge attempts and errors (debugging issuance). |

## Cache commands

| Command | Description |
|---|---|
| `pulsate cache stats [--cache <name>]` | Hit ratio, size, evictions, bytes saved. |
| `pulsate cache purge (--tag <t> \| --url <u> \| --prefix <p> \| --all) [--soft]` | Invalidate entries (propagates cluster-wide). `--soft` marks stale instead of deleting. |
| `pulsate cache warm <urls-file>` | Pre-populate the cache (e.g., post-deploy). |
| `pulsate cache inspect <key>` | Show a stored entry's metadata (validators, age, vary, tags). |

## Diagnostics & inspection

| Command | Description |
|---|---|
| `pulsate inspect [--filter host=...,path=...,status=5xx] [--for 30s]` | **Live request tap** to the terminal: per-request the matched route, each middleware decision, per-stage timings, upstream, and response — the CLI twin of the dashboard [request inspector](11-dashboard.md). |
| `pulsate doctor` | Environment & config health check: file descriptor limits, port availability, DNS, cert reachability, clock skew, kernel features (io_uring/SO_REUSEPORT), permission/capability issues — with fix suggestions. |
| `pulsate logs [--follow] [--filter ...]` | Stream/filter access & error logs from the running instance. |
| `pulsate routes` | Print the compiled routing table (precedence order) — see exactly how requests will match. |
| `pulsate upstreams` | Live upstream/target health, weights, breaker state, in-flight. |
| `pulsate top` | A `top`-like live TUI: rps, p99, errors, top routes/upstreams, cache hit ratio. |
| `pulsate trace <request-id>` | Pull the distributed trace for a request ID ([15. Observability](15-observability.md)). |

## Benchmark commands

| Command | Description |
|---|---|
| `pulsate bench <url> [--rate N] [--duration 30s] [--conns C] [--h2\|--h3]` | Built-in load generator with correct latency reporting (HdrHistogram, coordinated-omission aware) — quick local benchmarking without extra tools. |
| `pulsate bench --profile` | Run a load and capture a CPU flamegraph of the server ([10. Performance](10-performance.md)). |
| `pulsate bench compare <cfgA> <cfgB>` | A/B two configs under identical load. |

(For rigorous, reproducible benchmarking see [31. Benchmarking & Tuning](31-benchmarking-and-tuning.md).)

## Plugin commands

| Command | Description |
|---|---|
| `pulsate plugin new <name> [--lang rust\|go\|js]` | Scaffold a plugin project from the SDK template. |
| `pulsate plugin build` | Build the `.wasm` component. |
| `pulsate plugin test` | Run the plugin against a local mock-request harness. |
| `pulsate plugin run [--watch]` | Hot-load into a dev server for the local edit loop. |
| `pulsate plugin list` | Loaded plugins, versions, capabilities, and health on the running instance. |
| `pulsate plugin verify <file>` | Check signature/provenance and declared capabilities before trusting it. |

## Migration & import

| Command | Description |
|---|---|
| `pulsate import nginx <nginx.conf> [-o pulsate.flow]` | Convert an nginx config to Flow (mapping + fidelity warnings). |
| `pulsate import caddy <Caddyfile>` | Convert a Caddyfile. |
| `pulsate import haproxy <haproxy.cfg>` | Convert an HAProxy config (frontends/backends → sites + upstreams). |
| `pulsate import apache <httpd.conf>` | Convert an Apache vhost config (`ProxyPass`/`Redirect`/`DocumentRoot`). |
| `pulsate import --diff` | Show source→Flow mapping and any unsupported directives, without writing. |

Migration semantics and fidelity are detailed in [30. Migration & Import](30-migration-and-import.md).

## Upgrade & self-management

| Command | Description |
|---|---|
| `pulsate upgrade [--channel stable\|beta] [--check]` | Self-update the binary to the latest signed release (verifies signature/SBOM); `--check` only reports availability. |
| `pulsate upgrade --zero-downtime` | Binary upgrade with socket handoff to a new process while the old drains ([16. Deployment](16-deployment.md)). |
| `pulsate version [--json]` | Version, build info (commit, target), supported `flow_version` range, supported plugin ABI range. |
| `pulsate completion <shell>` | Generate shell completion (bash/zsh/fish/powershell). |
| `pulsate dashboard open` | Print a localhost dashboard URL with a short-lived token. |

## Exit codes & scripting

Stable, documented exit codes make Pulsate scriptable and CI-friendly (full list in [25. Error Catalog](25-error-and-status-catalog.md)):

| Code | Meaning |
|---|---|
| `0` | success |
| `1` | generic runtime error |
| `2` | config validation failed (`pulsate validate`) |
| `3` | could not bind / port in use |
| `4` | admin API unreachable |
| `5` | certificate/ACME error |
| `64` | usage error (bad flags) |

Combined with `--output json`, every command is automatable: `pulsate validate && pulsate reload`, `pulsate cert list -o json | jq ...`, etc.

## Cross-references
- [22. Admin API](22-admin-api.md) — the API the CLI drives for live operations.
- [02. Architecture](02-architecture.md) — reload/shutdown/snapshot semantics behind the commands.
- [14. Developer Experience](14-developer-experience.md) — `pulsate init`, app detection, dev mode.
- [30. Migration & Import](30-migration-and-import.md) — `pulsate import` details.
- [25. Error Catalog](25-error-and-status-catalog.md) — exit codes and error codes.
