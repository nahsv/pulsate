# pulsate-k8s

Kubernetes Gateway API controller: reconciles `GatewayClass`/`Gateway`/`HTTPRoute`
into a live Pulsate config through the same validate-then-atomic-swap path as an
admin reload.

Part of [Pulsate](https://github.com/squaretick/pulsate) — a reverse-proxy gateway in
one binary (TLS, caching, WAF, observability, admin API, WASM plugins). This crate
is a building block of the Pulsate workspace; most users want the `pulsate` binary
rather than this crate directly.

## License

Apache-2.0
