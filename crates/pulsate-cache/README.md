# pulsate-cache

HTTP caching: in-memory store, RFC-9111 freshness, validators, stale-while-revalidate, tag-based purge.

Part of [Pulsate](https://github.com/nahsv/pulsate) — a reverse-proxy gateway in
one binary (TLS, caching, WAF, observability, admin API, WASM plugins). This crate
is a building block of the Pulsate workspace; most users want the `pulsate` binary
rather than this crate directly.

## License

Apache-2.0
