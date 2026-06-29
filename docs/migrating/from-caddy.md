# Migrating from Caddy

Caddy and Pulsate share a philosophy — automatic HTTPS, a readable config — so
this is usually the smoothest migration. Auto-translate first:

```sh
p8 import caddy ./Caddyfile --diff
p8 import caddy ./Caddyfile -o pulsate.flow
```

## Side by side

```caddyfile
example.com, www.example.com {
    encode gzip

    handle_path /assets/* {
        root * /srv/app/public
        file_server
        header Cache-Control "public, max-age=31536000, immutable"
    }

    @ws path /cable
    reverse_proxy @ws 127.0.0.1:3000

    reverse_proxy 127.0.0.1:3000
}
```

becomes:

```flow
upstream app { target http://127.0.0.1:3000; policy least_conn }

site example.com www.example.com {
  tls auto                              # Caddy's automatic HTTPS, kept

  route /assets/* ~> strip_prefix("/assets")   # handle_path strips the prefix
                  ~> compress
                  ~> files("/srv/app/public")

  route /cable ~> ws(@app)
  route /*     ~> compress ~> proxy(@app)
}
```

## Directive mapping

| Caddyfile | Flow |
|-----------|------|
| site address block | `site … { … }` |
| automatic HTTPS (default) | `tls auto` (explicit) |
| `tls cert key` | `tls { cert "…" key "…" }` |
| `reverse_proxy host:port` | `upstream … { target … }` + `~> proxy(@…)` |
| `reverse_proxy` (WebSocket auto) | `~> ws(@…)` for upgrade routes |
| `file_server` + `root` | `files("dir")` |
| `handle_path /p/*` | `route /p/* ~> strip_prefix("/p") ~> …` |
| `handle /p/*` | `route /p/* ~> …` (no strip) |
| `encode gzip zstd` | `~> compress` |
| `header NAME value` | `~> headers(set={NAME: "value"})` |
| `redir` | `~> redirect(to="…", status=308)` |
| `basicauth` | `~> basic_auth(realm="…")` |
| `rate_limit` (plugin) | `~> rate_limit(N/min, key=ip)` (built in) |
| `forward_auth` | `~> forward_auth(…)` |
| matchers `@name` | route paths + middleware predicates |
| named `route` ordering | Flow precedence (exact > prefix > catch-all) |

## Gotchas

- **`handle_path` strips the matched prefix**; plain `handle` does not. Map the
  former to `strip_prefix("/p")`, the latter to a bare `route`.
- **Matcher ordering**: Caddy evaluates `handle` blocks in written order; Flow
  routes resolve by precedence. For overlapping paths, make the intent explicit
  with exact (`route = /x`) vs prefix (`route /x/*`) routes.
- **On-demand TLS / wildcard certs**: `tls auto` covers per-host issuance; for
  wildcards or DNS-01 see [09. Security](../09-security.md).

See the [universal cutover steps](README.md#the-universal-cutover-any-source).
