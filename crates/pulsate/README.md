# pulsate

A reverse-proxy gateway in one binary — automatic TLS, caching, a WAF, rate
limiting, observability, a loopback admin API and dashboard, and a sandboxed
WASM plugin host, configured by a single file.

```sh
cargo install pulsate            # installs the `pulsate` binary and its `pulsate` alias
pulsate up pulsate.flow
```

See the [project README](https://github.com/squaretick/pulsate) for the
configuration language, architecture, and the other install channels (Homebrew,
apt/dnf, Docker).
