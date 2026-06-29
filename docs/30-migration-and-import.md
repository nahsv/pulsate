# 30. Migration and Import

> The on-ramp from incumbents: `p8 import` tooling that converts nginx, Caddy, HAProxy, and Apache configurations into Pulsate Flow — with directive mapping tables, fidelity warnings, dry-run diffs, and round-trip validation. Lowering switching cost is a growth strategy.

**Contents**
- [Why importers matter](#why-importers-matter)
- [How import works](#how-import-works)
- [Fidelity model & warnings](#fidelity-model--warnings)
- [nginx → Flow](#nginx--flow)
- [Caddy → Flow](#caddy--flow)
- [HAProxy → Flow](#haproxy--flow)
- [Apache → Flow](#apache--flow)
- [Validation & rollout workflow](#validation--rollout-workflow)
- [Limitations](#limitations)
- [Cross-references](#cross-references)

---

## Why importers matter

The biggest barrier to adopting a new gateway is the config a team already has. `p8 import` turns "rewrite everything" into "review a generated file," making evaluation low-risk. Importers also *teach* the Flow language by example (you see your familiar config become Flow side by side). This is a deliberate adoption lever ([01. Vision](01-vision.md)).

## How import works

`pulsate-migrate` parses the foreign format into its own AST, maps it onto the Flow model, and renders `pulsate.flow`:

```
foreign config ─▶ [foreign parser] ─▶ foreign AST ─▶ [mapper] ─▶ Flow model
   ─▶ [Flow renderer + p8 fmt] ─▶ pulsate.flow  +  migration-report.md
```

```bash
p8 import nginx /etc/nginx/nginx.conf -o pulsate.flow     # write
p8 import nginx /etc/nginx/nginx.conf --diff            # preview mapping, write nothing
p8 import caddy ./Caddyfile
p8 import haproxy /etc/haproxy/haproxy.cfg              # frontends/backends → sites + upstreams
p8 import apache /etc/apache2/sites-enabled/site.conf  # <VirtualHost> → site
```

Every import emits a **migration report**: what mapped cleanly, what needed approximation, and what couldn't be represented (with line references back to the source).

## Fidelity model & warnings

Each mapped directive is classified:
- **Exact** — semantically identical in Flow (e.g., `proxy_pass` → `proxy(@u)`).
- **Approximate** — mapped with a behavior note (e.g., nginx's implicit caching headers vs Flow's explicit `cache`); a `# MIGRATION:` comment is left inline.
- **Manual** — cannot be auto-translated (embedded Lua, custom C modules, obscure directives); emitted as a `# TODO(migration):` stub with a doc link.
- **Dropped** — irrelevant to Pulsate (e.g., `worker_processes` maps to `pulsate { workers }`, but nginx event-module tuning is dropped with a note).

The report tallies counts per class so you know the manual effort at a glance.

## nginx → Flow

Mapping highlights:

| nginx | Flow |
|---|---|
| `server { listen 443 ssl; server_name x; }` | `site x { tls {...} }` |
| `location /api { proxy_pass http://b; }` | `route /api/* ~> proxy(http://b)` |
| `location = /h { ... }` | `route = /h ~> ...` |
| `location ~ ^/u/(\d+)` | `route ~ ^/u/(\d+) ~> ...` |
| `upstream b { server a:80 weight=3; least_conn; }` | `upstream b { target http://a:80 weight=3; policy least_conn }` |
| `proxy_set_header H v;` | `headers(set={H: "v"})` |
| `add_header H v;` | `headers(set={H: "v"})` (Egress) |
| `limit_req zone=...` | `rate_limit(<rate>, key=...)` |
| `proxy_cache ...; proxy_cache_valid` | `cache(@c)` + a `cache {}` block |
| `gzip on;` | `compress` |
| `ssl_certificate / _key` | `tls { cert; key }` |
| `return 301 https://...` | `redirect(to=..., status=301)` |
| `try_files $uri /index.html` | `files("...", try=[...])` |

Notes: nginx `if`/rewrite chains map to `rewrite`/`require` where expressible, else flagged Manual; `map` blocks become `let`/predicates where possible; Lua (`content_by_lua`) is always Manual (suggest a WASM plugin).

## Caddy → Flow

Caddy is the closest in spirit (auto-TLS by default), so mappings are mostly Exact:

| Caddyfile | Flow |
|---|---|
| `example.com { ... }` | `site example.com { tls auto; ... }` |
| `reverse_proxy b:80` | `route /* ~> proxy(http://b:80)` |
| `handle /api/* { reverse_proxy ... }` | `route /api/* ~> proxy(...)` |
| `file_server` / `root` | `files("<root>")` |
| `encode gzip zstd` | `compress(gzip, zstd)` |
| `header H v` | `headers(set={H:"v"})` |
| `tls email@x` / `tls internal` | `acme { email }` / `tls` (dev CA) |
| `basicauth` | `basic_auth(users=@set)` |
| matchers `@name path /x` | route predicates `[...]` |

Caddy's JSON config is also importable (the underlying model), and Caddy plugins map to Pulsate plugins/built-ins where equivalents exist (else Manual).

## HAProxy → Flow

HAProxy splits routing across `frontend`/`backend`/`listen` sections; the importer
turns server pools into upstreams and binds + routing rules into sites:

| HAProxy | Flow |
|---|---|
| `backend b { server s 10.0.0.1:8080 }` | `upstream b { target http://10.0.0.1:8080 }` |
| `frontend f` / `listen f` | `site ... { ... }` |
| `bind *:443 ssl crt ...` | `tls auto` |
| `acl is_api path_beg /api` + `use_backend api if is_api` | `route /api/* ~> proxy(@api)` |
| `acl exact path /h` + `use_backend ...` | `route = /h ~> proxy(@...)` |
| `default_backend web` | `route /* ~> proxy(@web)` |
| `acl host hdr(host) example.com` | site host `example.com` |
| `redirect location <url> code 301` | `redirect(to="<url>", status=301)` |

A `listen` section that carries its own `server` lines becomes both an upstream and
a site proxying to it. Non-path ACLs (header/source matches), stick tables, and
TCP-mode rules are flagged Manual.

## Apache → Flow

The importer reads `<VirtualHost>` blocks and maps their reverse-proxy, static, and
redirect directives:

| Apache | Flow |
|---|---|
| `<VirtualHost *:443>` + `SSLEngine on` | `site ... { tls auto }` |
| `ServerName x` / `ServerAlias y z` | `site x y z` |
| `ProxyPass /api http://b:8080/` | `route /api/* ~> proxy(http://b:8080)` |
| `ProxyPass / http://b:3000/` | `route /* ~> proxy(http://b:3000)` |
| `DocumentRoot /srv/www` | `route /* ~> files("/srv/www")` (when nothing else claims `/`) |
| `Redirect 301 /old <url>` | `route /old ~> redirect(to="<url>", status=301)` |

More specific `ProxyPass` paths are emitted before the catch-all so they match first.
`ProxyPassReverse` is implied by `proxy()` and dropped; `ProxyPassMatch`,
`RedirectMatch`, `RewriteRule`, and `<Directory>`/`<Location>` containers are flagged
Manual.

## Validation & rollout workflow

A safe, reviewable migration path:
1. `p8 import <kind> <src> --diff` — review the mapping and report; no files written.
2. `p8 import <kind> <src> -o pulsate.flow` — generate; **review the `# MIGRATION:`/`# TODO:` notes**.
3. `p8 validate pulsate.flow` — catch anything that needs fixing (`PLS-CFG-*`).
4. **Shadow/parallel run:** run Pulsate alongside the old proxy on a different port; mirror or canary a slice of traffic; compare responses/metrics.
5. **Round-trip check:** `p8 import` can optionally re-derive the intended behavior and diff against the source's observed routing for representative requests.
6. Cut over with a weighted/canary rollout ([06. Reverse Proxy](06-reverse-proxy.md)); keep the old config for rollback.

## Limitations

Honest boundaries (also stated in the report):
- **Embedded code** (nginx Lua, Caddy/Apache modules, HAProxy Lua) cannot be auto-translated — flagged Manual with a suggestion to port to a WASM plugin ([12. Plugins](12-plugins.md)).
- **Imperative `if`/rewrite spaghetti** may need human simplification; the importer prefers correctness-with-a-warning over a clever-but-wrong translation.
- **Exotic/rare directives** are emitted as TODO stubs rather than guessed.
- The importer **never silently drops** behavior — anything not represented is reported. The goal is a trustworthy 80–95% automatic conversion plus a clear list of the remainder.

## Cross-references
- [13. CLI](13-cli.md) — the `p8 import` commands.
- [04. Configuration](04-configuration.md) & [27. Config Reference](27-config-reference.md) — the Flow targets of mapping.
- [06. Reverse Proxy](06-reverse-proxy.md) — canary cutover.
- [12. Plugins](12-plugins.md) — porting embedded logic to WASM.
