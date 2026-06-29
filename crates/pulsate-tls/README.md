# pulsate-tls

rustls server configuration: SNI certificate resolution, ALPN, manual certs (mTLS to follow).

Part of [Pulsate](https://github.com/squaretick/pulsate) — a reverse-proxy gateway in
one binary (TLS, caching, WAF, observability, admin API, WASM plugins). This crate
is a building block of the Pulsate workspace; most users want the `pulsate` binary
rather than this crate directly.

## License

Apache-2.0
