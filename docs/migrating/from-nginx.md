# Migrating from nginx

Auto-translate first, then read this to understand and tune the result:

```sh
pulsate import nginx /etc/nginx/nginx.conf --diff     # preview
pulsate import nginx /etc/nginx/nginx.conf -o pulsate.flow
```

## Side by side

A typical nginx reverse proxy + static site + TLS:

```nginx
upstream app { server 127.0.0.1:3000; }

server {
    listen 80;
    server_name example.com www.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name example.com www.example.com;

    ssl_certificate     /etc/letsencrypt/live/example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/example.com/privkey.pem;

    gzip on;

    location /assets/ {
        root /srv/app/public;
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    location /cable {
        proxy_pass http://app;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }

    location / {
        proxy_pass http://app;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        limit_req zone=api burst=20;
    }
}
```

becomes:

```flow
upstream app { target http://127.0.0.1:3000; policy least_conn }
cache assets { store memory { max 256MB }; default_ttl 1y; stale_while_revalidate 60s }

site example.com www.example.com {
  tls auto                                  # replaces ssl_certificate + certbot

  route /assets/* ~> cache(@assets)
                  ~> compress
                  ~> files("/srv/app/public")

  route /cable ~> ws(@app)                  # Upgrade/Connection handled for you

  route /* ~> rate_limit(600/min, key=ip)   # replaces limit_req
           ~> compress                       # replaces gzip on
           ~> proxy(@app)                     # X-Forwarded-* added automatically
}
```

The plain-HTTP→HTTPS redirect server block is gone: Pulsate redirects HTTP to
HTTPS automatically when `tls auto` is set.

## Directive mapping

| nginx | Flow |
|-------|------|
| `upstream { server … }` | `upstream NAME { target … }` |
| `proxy_pass http://app` | `~> proxy(@app)` |
| `listen 443 ssl http2` + `ssl_certificate*` | `tls auto` (or `tls { cert … key … }`) |
| `server_name a b` | `site a b { … }` |
| `location /p/` | `route /p/* ~> …` |
| `location = /p` | `route = /p ~> …` |
| `root` / `try_files` | `files("dir", try=[…])` |
| `gzip on` | `~> compress` |
| `add_header` / `proxy_set_header` | `~> headers(set={…})` |
| `limit_req` | `~> rate_limit(N/min, key=ip)` |
| `proxy_set_header Upgrade …` (WebSocket) | `~> ws(@app)` |
| `return 301 …` | `~> redirect(to="…", status=308)` |
| `auth_basic` | `~> basic_auth(realm="…")` |
| `auth_request` | `~> forward_auth(…)` |
| `proxy_cache` | `cache NAME { … }` + `~> cache(@NAME)` |
| `allow`/`deny` | `~> require(ip=[…])` |
| `worker_processes` | `pulsate { workers N }` |
| `content_by_lua` / `njs` | **Manual** — port to a WASM plugin |

## Gotchas

- **`X-Forwarded-*` are automatic** on `proxy(...)` — don't re-add them, and set
  your app to trust the loopback proxy.
- **nginx `location` longest-prefix matching** maps to Flow's deterministic
  precedence (exact > longest-prefix > catch-all); usually identical, but review
  any configs that relied on regex `location` ordering.
- **`if` blocks / rewrite chains** become `rewrite`/`require` where expressible,
  else the importer flags them `# MIGRATION: manual`.
- **Lua** has no auto-translation — the importer suggests a WASM plugin
  ([12. Plugins](../12-plugins.md)).

See the [universal cutover steps](README.md#the-universal-cutover-any-source).
