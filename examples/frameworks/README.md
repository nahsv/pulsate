# Framework recipes

Copy-paste-ready `pulsate.flow` configs for putting Pulsate in front of common
app frameworks. Each file is self-contained and starts with numbered setup
steps (run the app server on loopback → point DNS → `p8 up`). TLS is automatic
via Let's Encrypt — no certbot, no cron.

| Framework | File | Notes |
|-----------|------|-------|
| Ruby on Rails | [`rails.flow`](rails.flow) | Puma, fingerprinted assets, ActionCable WS |
| Next.js | [`nextjs.flow`](nextjs.flow) | SSR/ISR, immutable `/_next/static` |
| Django | [`django.flow`](django.flow) | Gunicorn/Uvicorn, static+media, locked `/admin` |
| Laravel | [`laravel.flow`](laravel.flow) | Octane/FrankenPHP, Vite build assets |
| Express / Node | [`express.flow`](express.flow) | Multi-worker LB, Socket.IO, CORS |
| Phoenix / Elixir | [`phoenix.flow`](phoenix.flow) | LiveView + Channels WebSockets |
| FastAPI / Python | [`fastapi.flow`](fastapi.flow) | Uvicorn ASGI, CORS, rate limit, WS |
| Spring Boot / Java | [`spring-boot.flow`](spring-boot.flow) | Locked Actuator, STOMP WS |
| Go (net/http) | [`go.flow`](go.flow) | Plain HTTP on loopback, optional gRPC |
| SvelteKit / Vite | [`sveltekit.flow`](sveltekit.flow) | Static SPA fallback *or* Node SSR |

## The pattern they all share

```flow
upstream app { target http://127.0.0.1:PORT; policy least_conn }

site example.com {
  tls auto                              # automatic HTTPS
  route /assets/* ~> files("public")    # static straight from disk
  route /ws       ~> ws(@app)           # websockets
  route /*        ~> compress ~> proxy(@app)
}
```

1. **Run your app server on loopback** (127.0.0.1) — Pulsate terminates TLS, so
   the app speaks plain HTTP.
2. **Trust the proxy** in your framework so it reads `X-Forwarded-Proto`/`-For`
   (each recipe notes the exact setting).
3. **Validate then serve:** `p8 validate app.flow && sudo p8 up app.flow`.

## Migrating from another proxy?

See [`docs/migrating/`](../../docs/migrating/) for side-by-side configs and
step-by-step cutovers from nginx, Caddy, Apache, Thruster, Traefik, and HAProxy
— or run `p8 import nginx /etc/nginx/nginx.conf` to auto-translate.
