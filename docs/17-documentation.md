# 17. Documentation

> How Pulsate's documentation is structured so that a newcomer succeeds in minutes and an expert finds the exact reference they need — organized by the Diátaxis model (tutorials, how-tos, reference, explanation), plus examples, architecture docs, the contribution guide, and the API reference.

**Contents**
- [Documentation philosophy](#documentation-philosophy)
- [Information architecture (Diátaxis)](#information-architecture-diátaxis)
- [Tutorials](#tutorials)
- [How-to guides](#how-to-guides)
- [Reference](#reference)
- [Explanation / architecture docs](#explanation--architecture-docs)
- [Examples](#examples)
- [API reference](#api-reference)
- [Contribution guide](#contribution-guide)
- [Tooling & quality](#tooling--quality)
- [Cross-references](#cross-references)

---

## Documentation philosophy

Docs are a feature with the same bar as code: **a user should never have to read the source to use Pulsate correctly.** Three commitments:
1. **Docs match the binary.** Reference docs are generated from the same definitions the binary uses (config schema, CLI, metrics, errors), so they cannot drift. A release with undocumented config keys fails CI.
2. **The 30-second path is the front door.** The landing tutorial gets you to HTTPS-serving in under a minute; depth is reachable but never in the way.
3. **Every error points to a doc.** Error codes (`PLS-*`) and CLI messages link to the exact page that explains and fixes them ([25. Error Catalog](25-error-and-status-catalog.md)).

## Information architecture (Diátaxis)

Pulsate adopts the **Diátaxis** framework — four distinct doc types serving four distinct needs — because mixing them is the usual cause of bad docs:

| Type | Purpose | Reader question |
|---|---|---|
| **Tutorials** | learning by doing | "get me started" |
| **How-to guides** | accomplish a task | "how do I do X?" |
| **Reference** | look up facts | "what is the exact syntax/value?" |
| **Explanation** | understand concepts | "why does it work this way?" |

```
pulsate.nahsv.com/
├── start/            tutorials (learning-oriented)
├── guides/           how-tos (task-oriented)
├── reference/        config / CLI / API / metrics / errors (information-oriented)
├── concepts/         explanation & architecture (understanding-oriented)
├── examples/         runnable, copy-pasteable configs & repos
├── plugins/          SDK, cookbook, marketplace
└── contributing/     contributor & governance docs
```

## Tutorials

Learning-oriented, hand-held, guaranteed to work:
- **"Your first gateway in 60 seconds"** — install → `p8 init` → `p8 up` → HTTPS.
- **Framework quickstarts** — Rails, Node/Next, Go, Rust, static/SPA (one per stack, mirrors [14. DX](14-developer-experience.md)).
- **"From laptop to production"** — the same config from dev to a VM to Kubernetes.
- **"Build your first plugin"** — scaffold → middleware → test → load ([12. Plugins](12-plugins.md)).
Each tutorial is end-to-end, tested in CI against the current binary, and ends with "where to go next."

## How-to guides

Task-oriented recipes for real problems:
- Enable caching for an API; add a WAF; rate-limit by API key; set up mTLS; do a weighted canary; serve gRPC/WebSocket; wildcard certs via DNS-01; run behind a cloud LB (PROXY protocol); migrate from nginx/Caddy/Traefik; set up Prometheus + Grafana + tracing; cluster three nodes; rotate secrets.
Short, imperative, with copy-pasteable Flow snippets and a "verify it worked" step.

## Reference

Information-oriented, exhaustive, generated where possible:
- **Configuration reference** — every block/directive/field ([27. Config Reference](27-config-reference.md)), generated from the config schema.
- **CLI reference** — every command/flag ([13. CLI](13-cli.md)), generated from the `clap` definitions.
- **Admin API reference** — OpenAPI/gRPC ([22. Admin API](22-admin-api.md)), generated from the API spec.
- **Metrics catalog** ([26](26-metrics-and-slo-catalog.md)) and **error catalog** ([25](25-error-and-status-catalog.md)) — generated from the registries.
Generation guarantees these never lie about the running binary.

## Explanation / architecture docs

Understanding-oriented — the "why":
- The **concepts** section adapts this implementation plan's architecture material ([02. Architecture](02-architecture.md)): the control/data-plane split, the snapshot model, the request lifecycle, the middleware pipeline.
- **Architecture Decision Records** ([24. ADRs](24-architecture-decision-records.md)) are published so users and contributors see the reasoning behind major choices.
- **Threat model** ([21](21-threat-model.md)) and security posture are documented openly (security through transparency, not obscurity).

## Examples

- A `examples/` directory in the repo and a docs gallery: minimal, SPA+API, microservices gateway, hardened public API, multi-tenant, Kubernetes, Compose, plugin examples — each a runnable `pulsate.flow` (+ app where relevant), CI-tested so they always work.
- Examples are cross-linked from the relevant reference/how-to pages and from [04. Configuration](04-configuration.md).

## API reference

- **Rust API docs** (`cargo doc`) for `pulsate-core` and `pulsate-sdk` — the public, semver-stable surface for embedders and plugin authors — published per release, with doc-tests that run in CI.
- **Admin API** as OpenAPI 3 + gRPC reflection, rendered as interactive docs ([22. Admin API](22-admin-api.md)).
- **Plugin ABI** (WIT) reference with per-world docs and language-binding guides ([12. Plugins](12-plugins.md)).

## Contribution guide

In `contributing/` (and `CONTRIBUTING.md`):
- **Getting set up:** toolchain, `cargo xtask` dev tasks, running the test suites ([28. Testing](28-testing-and-conformance.md)).
- **Code standards & conventions** ([03. Repository](03-repository.md)).
- **The RFC process** for substantial changes ([18. Open Source](18-open-source.md)).
- **How to add:** a middleware, a config directive (and its generated docs), a metric, an error code — each with the checklist that keeps docs/tests in sync.
- **Good first issues**, the issue/PR templates, and the Code of Conduct.

## Tooling & quality

- **Site:** a static docs site (e.g., mdBook or a Docusaurus-style generator) built in CI, versioned per release (so docs for 1.2 stay available), with full-text and **AI-answer-friendly** structure ([20. Future](20-future.md)).
- **Doc tests:** code/config snippets are extracted and validated (`p8 validate`/`p8 fmt --check`/`cargo test --doc`) so no example rots.
- **Link checking & coverage:** broken internal links fail CI; a "doc coverage" check ensures every public config key, CLI command, metric, and error code has a reference entry.
- **Localization-ready** structure for future translation.

## Cross-references
- [13. CLI](13-cli.md), [22. Admin API](22-admin-api.md), [27. Config Reference](27-config-reference.md) — generated reference sources.
- [25. Error Catalog](25-error-and-status-catalog.md), [26. Metrics Catalog](26-metrics-and-slo-catalog.md) — generated catalogs.
- [18. Open Source](18-open-source.md) — contribution process, RFCs, governance.
- [14. Developer Experience](14-developer-experience.md) — the quickstarts docs mirror.
- [24. ADRs](24-architecture-decision-records.md) — published decision records.
