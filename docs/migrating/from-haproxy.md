# Migrating from HAProxy

HAProxy's frontend/backend/ACL model maps to Flow's site/upstream/route; the
load-balancing knobs translate directly.

> **An automatic importer now exists:** `pulsate import haproxy <haproxy.cfg>`. It maps
> `backend` server pools → `upstream { target ... }`, `frontend`/`listen` binds →
> `site`, `bind ... ssl` → `tls auto`, path-based `acl` + `use_backend` → path
> routes, and `default_backend` → a catch-all route. Constructs without a clean
> equivalent — non-path ACLs, stick-tables, and `mode tcp` (L4) — are flagged for
> manual review. The walkthrough below is the deeper reference for those cases.

## Side by side

```haproxy
frontend www
    bind :80
    bind :443 ssl crt /etc/haproxy/certs/example.com.pem alpn h2,http/1.1
    http-request redirect scheme https unless { ssl_fc }

    acl is_api path_beg /api
    use_backend api if is_api
    default_backend web

backend api
    balance leastconn
    option httpchk GET /health
    server a1 127.0.0.1:8000 check
    server a2 127.0.0.1:8001 check

backend web
    balance roundrobin
    server w1 127.0.0.1:3000 check
```

becomes:

```flow
upstream api {
  target http://127.0.0.1:8000
  target http://127.0.0.1:8001
  policy least_conn
  health { path "/health" }
}
upstream web {
  target http://127.0.0.1:3000
  policy round_robin
}

site example.com {
  tls auto                                   # bind ssl crt … + HTTP→HTTPS redirect

  route /api/* ~> proxy(@api)                # use_backend api if is_api
  route /*     ~> proxy(@web)                # default_backend web
}
```

## Mapping

| HAProxy | Flow |
|---------|------|
| `frontend` + `bind` | `site … { … }` (ports default 80/443) |
| `bind :443 ssl crt …` | `tls auto` (or `tls { cert … key … }`) |
| `http-request redirect scheme https` | automatic with `tls auto` |
| `backend` + `server …` | `upstream NAME { target … }` |
| `balance roundrobin` | `policy round_robin` |
| `balance leastconn` | `policy least_conn` |
| `balance source` | `policy ip_hash` |
| `server … weight N` | `target … weight=N` |
| `option httpchk` | `upstream { health { path "…" } }` |
| `server … check` (ejection) | passive ejection + `retry { attempts N }` |
| `acl … path_beg /api` + `use_backend` | `route /api/* ~> proxy(@api)` |
| `acl … hdr(host)` | separate `site` blocks |
| `http-request deny` (ACL) | `~> require(ip=[…])` / `respond(status=403)` |
| `http-request set-header` | `~> headers(set={…})` |
| `stick-table` rate limiting | `~> rate_limit(N/min, key=ip)` |
| `mode tcp` (L4) | not a Flow use case — Pulsate is an L7 HTTP gateway |

## Gotchas

- **L4 / `mode tcp`**: Pulsate is an HTTP(S) application gateway, not a generic
  TCP load balancer. Keep HAProxy (or another L4 LB) for raw TCP/stream services;
  use Pulsate for HTTP.
- **ACL richness**: complex ACL expressions may need splitting across routes;
  host-based ACLs become separate `site` blocks.
- **Health checks**: HAProxy actively probes; Pulsate combines a configurable
  health path with passive ejection on errors — review thresholds in
  [06. Reverse Proxy](../06-reverse-proxy.md).

See the [universal cutover steps](README.md#the-universal-cutover-any-source).
