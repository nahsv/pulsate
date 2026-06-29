# Migrating to Pulsate

Moving off an existing proxy? These guides show your current config and the
equivalent Pulsate **Flow**, side by side, plus a safe step-by-step cutover.

Most configs can be auto-translated first, then hand-tuned:

```sh
pulsate import nginx   /etc/nginx/nginx.conf --diff   # preview, write nothing
pulsate import nginx   /etc/nginx/nginx.conf -o pulsate.flow
pulsate import caddy   ./Caddyfile           -o pulsate.flow
pulsate import haproxy /etc/haproxy/haproxy.cfg -o pulsate.flow
pulsate import apache  ./site.conf           -o pulsate.flow
```

The importer annotates anything it can't translate 1:1 with a `# MIGRATION:`
comment, so you always know what to review. See
[30. Migration and Import](../30-migration-and-import.md) for the fidelity model.

## Guides

| From | Guide | Auto-import |
|------|-------|-------------|
| nginx | [from-nginx.md](from-nginx.md) | ✅ `pulsate import nginx` |
| Caddy | [from-caddy.md](from-caddy.md) | ✅ `pulsate import caddy` |
| Apache httpd | [from-apache.md](from-apache.md) | ✅ `pulsate import apache` |
| Thruster (Rails) | [from-thruster.md](from-thruster.md) | manual (guide) |
| Traefik | [from-traefik.md](from-traefik.md) | manual (guide) |
| HAProxy | [from-haproxy.md](from-haproxy.md) | ✅ `pulsate import haproxy` |

## The universal cutover (any source)

1. **Translate** your config to `pulsate.flow` (import or by hand from the guide).
2. **Validate:** `pulsate validate pulsate.flow` — typed errors point at line/column.
3. **Dry-run on a high port**, old proxy still serving 80/443:
   `pulsate up pulsate.flow --listen 127.0.0.1:8443` and curl it directly.
4. **Diff behavior:** compare headers/status/bodies against the old proxy for
   your top routes (`curl -I`, asset URLs, redirects, auth).
5. **Cut over:** stop the old proxy, then `sudo pulsate up pulsate.flow` on 80/443.
   Pulsate provisions TLS automatically (Let's Encrypt) — no certbot to port.
6. **Rollback:** keep the old config; if anything is off, stop Pulsate and start
   the old proxy. Nothing in Pulsate mutates your app or certs irreversibly.

## What changes conceptually

- **TLS is automatic.** Delete your `ssl_certificate`/certbot/`tls` cert plumbing
  — `tls auto` provisions and renews for you.
- **Config is typed, not templated.** Durations (`30s`), sizes (`10MB`), rates
  (`100/min`), and `@references` are first-class; mistakes fail validation
  instead of 500ing at runtime.
- **Routing is explicit and ordered** by precedence (exact > longest-prefix >
  catch-all) — no surprise `location` longest-match quirks.
- **Middleware is a left-to-right pipeline** (`~>`), so the request path reads
  top to bottom.
