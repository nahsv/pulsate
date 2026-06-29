# Migrating from Apache httpd

> **An automatic importer now exists:** `pulsate import apache <httpd.conf>`. It maps
> `<VirtualHost>` `ServerName`/`ServerAlias` → `site`, `SSLEngine`/`:443` →
> `tls auto`, `ProxyPass` → `proxy(...)`, `DocumentRoot` → `files(...)`, and
> `Redirect` → `redirect(...)`. Directives that don't translate 1:1 —
> `ProxyPassMatch`, `RewriteRule`, and `<Directory>` blocks — are flagged for
> manual review. The walkthrough below is the deeper reference for those cases.

The mapping is mechanical — most `mod_proxy` + `mod_ssl` vhosts become a few
Flow lines.

## Side by side

```apache
<VirtualHost *:80>
    ServerName example.com
    Redirect permanent / https://example.com/
</VirtualHost>

<VirtualHost *:443>
    ServerName example.com
    ServerAlias www.example.com

    SSLEngine on
    SSLCertificateFile      /etc/ssl/example.com.crt
    SSLCertificateKeyFile   /etc/ssl/example.com.key

    ProxyPreserveHost On
    ProxyPass        /assets/ !
    Alias /assets/ /srv/app/public/assets/
    <Directory /srv/app/public/assets/>
        Header set Cache-Control "public, max-age=31536000, immutable"
        Require all granted
    </Directory>

    ProxyPass        / http://127.0.0.1:3000/
    ProxyPassReverse / http://127.0.0.1:3000/
    RequestHeader set X-Forwarded-Proto "https"

    <Location /admin>
        AuthType Basic
        Require valid-user
    </Location>
</VirtualHost>
```

becomes:

```flow
upstream app { target http://127.0.0.1:3000; policy least_conn }
cache assets { store memory { max 256MB }; default_ttl 1y; stale_while_revalidate 60s }

site example.com www.example.com {
  tls auto                                   # SSLEngine + cert files + certbot

  route /assets/* ~> cache(@assets)
                  ~> compress
                  ~> files("/srv/app/public/assets")

  route /admin ~> basic_auth(realm="admin") ~> proxy(@app)

  route /* ~> compress ~> proxy(@app)        # ProxyPass/ProxyPassReverse + X-Forwarded-*
}
```

The `:80` redirect vhost disappears — `tls auto` redirects HTTP→HTTPS for you.

## Directive mapping

| Apache | Flow |
|--------|------|
| `<VirtualHost>` + `ServerName`/`ServerAlias` | `site a b { … }` |
| `SSLEngine on` + `SSLCertificate*` | `tls auto` (or `tls { cert … key … }`) |
| `ProxyPass / http://…` + `ProxyPassReverse` | `~> proxy(@app)` |
| `ProxyPreserveHost On` | default (Host is preserved) |
| `Alias` + `<Directory>` | `files("dir")` |
| `mod_deflate` (`AddOutputFilterByType`) | `~> compress` |
| `Header set` | `~> headers(set={…})` |
| `Redirect permanent` | `~> redirect(to="…", status=308)` |
| `RewriteRule` | `~> rewrite(…)` (simple cases) / Manual |
| `AuthType Basic` + `Require valid-user` | `~> basic_auth(realm="…")` |
| `Require ip 10.0.0.0/8` | `~> require(ip=["10.0.0.0/8"])` |
| `mod_security` rules | `~> waf(…)` (built-in signature WAF) |
| `RewriteRule … [P]` WebSocket | `~> ws(@app)` |
| `.htaccess` | no equivalent — fold rules into the `site` block |

## Gotchas

- **`.htaccess` is not supported** — Apache's per-directory override model has no
  Flow analogue. Move those rules into the `site`/`route` config (this is usually
  a simplification).
- **`mod_rewrite`** beyond simple path rewrites is flagged Manual; express intent
  with `rewrite`/`redirect`/`require`.
- **MPM/worker tuning** (`StartServers`, `MaxRequestWorkers`) has no 1:1 — Pulsate
  is async; set `pulsate { workers N }` only if you need to pin it.

See the [universal cutover steps](README.md#the-universal-cutover-any-source).
