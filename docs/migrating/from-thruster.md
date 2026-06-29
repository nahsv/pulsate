# Migrating from Thruster (Rails)

[Thruster](https://github.com/basecamp/thruster) wraps a Rails Puma process to
add HTTP/2, TLS, gzip, and X-Sendfile asset acceleration. Pulsate does the same
job and adds caching, a WAF, rate limiting, observability, and clustering — as a
config file instead of env vars.

Thruster has no config file, so there's no importer; map its environment
variables to Flow directives.

## Side by side

Thruster (env-driven, wrapping `bin/thrust bin/rails server`):

```bash
TLS_DOMAIN=example.com \
HTTP_PORT=80 HTTPS_PORT=443 \
TARGET_PORT=3000 \
X_SENDFILE_ENABLED=1 \
CACHE_SIZE=67108864 \
MAX_REQUEST_BODY=10485760 \
  bin/thrust bin/rails server -p 3000
```

becomes — run Rails normally on loopback, put Pulsate in front:

```flow
# bin/rails server -b 127.0.0.1 -p 3000 -e production
upstream puma { target http://127.0.0.1:3000; policy least_conn }

cache assets { store memory { max 256MB }; default_ttl 1y; stale_while_revalidate 60s }

site example.com {
  tls auto                                   # TLS_DOMAIN + automatic certs

  # X-Sendfile acceleration -> serve fingerprinted assets from disk directly.
  route /assets/* ~> cache(@assets)
                  ~> compress
                  ~> files("public")

  route /cable ~> ws(@puma)
  route /*     ~> compress ~> proxy(@puma)    # gzip + HTTP/2 included
}
```

## Variable mapping

| Thruster env | Flow |
|--------------|------|
| `TLS_DOMAIN` | `site DOMAIN { tls auto }` |
| `HTTP_PORT` / `HTTPS_PORT` | default 80/443 (override with `--listen`) |
| `TARGET_PORT` | `upstream { target http://127.0.0.1:PORT }` |
| HTTP/2 (always on) | automatic on `tls auto` |
| gzip (always on) | `~> compress` |
| `X_SENDFILE_ENABLED` | `~> files("public")` on the asset route |
| `CACHE_SIZE` | `cache NAME { store memory { max … } }` |
| `MAX_REQUEST_BODY` | `pulsate { max_body 10MB }` (or per-route) |
| `BAD_GATEWAY_PAGE` | `~> on_error(...)` / `respond(...)` |

## Why move

Thruster is deliberately minimal (one app, one process). Reach for Pulsate when
you want any of: multiple apps/domains on one host, response caching, a WAF and
rate limiting, Prometheus metrics + access logs, blue-green/canary upstreams, or
a cluster — without bolting on a second tool. The Rails recipe in
[`examples/frameworks/rails.flow`](../../examples/frameworks/rails.flow) is a
ready starting point.

See the [universal cutover steps](README.md#the-universal-cutover-any-source).
