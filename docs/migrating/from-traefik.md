# Migrating from Traefik

Traefik's router/service/middleware model maps cleanly onto Flow's
route/upstream/pipeline.

> **No automatic Traefik importer yet.** Unlike nginx, Caddy, HAProxy, and
> Apache, there is no `pulsate import traefik` command — this is a manual migration
> guide. If you also run one of those sources, `pulsate import nginx|caddy|haproxy|apache`
> can translate it for you; see [29/30](../30-migration-and-import.md). The
> mapping below is mechanical, so the by-hand translation is usually short.

## Side by side

Traefik dynamic config (file provider):

```yaml
http:
  routers:
    app:
      rule: "Host(`example.com`) && PathPrefix(`/`)"
      entryPoints: [websecure]
      service: app
      middlewares: [ratelimit, compress]
      tls:
        certResolver: letsencrypt
  middlewares:
    ratelimit:
      rateLimit: { average: 10, burst: 20 }
    compress:
      compress: {}
  services:
    app:
      loadBalancer:
        servers:
          - url: "http://127.0.0.1:3000"
          - url: "http://127.0.0.1:3001"
```

becomes:

```flow
upstream app {
  target http://127.0.0.1:3000
  target http://127.0.0.1:3001
  policy round_robin
}

site example.com {
  tls auto                                   # certResolver: letsencrypt

  route /* ~> rate_limit(600/min, key=ip)    # middlewares: ratelimit
           ~> compress                        # middlewares: compress
           ~> proxy(@app)                      # service: app loadBalancer
}
```

## Mapping

| Traefik | Flow |
|---------|------|
| `router.rule: Host(...)` | `site … { … }` |
| `router.rule: PathPrefix(/p)` | `route /p/* ~> …` |
| `router.rule: Path(/p)` | `route = /p ~> …` |
| `service.loadBalancer.servers` | `upstream { target … target … }` |
| LB strategy (wrr / round-robin) | `policy round_robin` / `weight=N` on targets |
| `tls.certResolver` (ACME) | `tls auto` |
| middleware `compress` | `~> compress` |
| middleware `rateLimit` | `~> rate_limit(N/min, key=ip)` |
| middleware `headers` | `~> headers(set={…})` |
| middleware `stripPrefix` | `~> strip_prefix("/p")` |
| middleware `redirectScheme`/`redirectRegex` | `~> redirect(…)` (HTTP→HTTPS is automatic) |
| middleware `basicAuth` | `~> basic_auth(realm="…")` |
| middleware `forwardAuth` | `~> forward_auth(…)` |
| middleware `ipWhiteList` | `~> require(ip=[…])` |
| middleware chain (ordered list) | `~>` pipeline (left to right) |
| `loadBalancer.healthCheck` | `upstream { … }` passive ejection / `retry` |

## Gotchas

- **Provider model**: Traefik often discovers config from Docker/K8s labels. On
  Kubernetes, prefer the Gateway API controller (`pulsate-k8s`) over translating
  labels by hand — see [29/30](../30-migration-and-import.md).
- **Middleware order matters** in both systems; the `~>` chain reads in execution
  order, so keep the same sequence.
- **Rule combinations** (`||`, regexp host rules) may need splitting into multiple
  `site`/`route` entries; translate anything non-1:1 by hand.

See the [universal cutover steps](README.md#the-universal-cutover-any-source).
