# Contributing

Thanks for your interest in Pulsate. Governance, the RFC process, and the
security-disclosure policy are described in
[docs/18-open-source.md](docs/18-open-source.md).

## Ground rules

These are enforced in CI, so check them locally before opening a PR:

- `#![forbid(unsafe_code)]` by default. `unsafe` is allowed only in a small set of
  reviewed, test-covered modules, each behind a `// SAFETY:` comment.
- No panics on request paths. Fallible paths return `Result<_, PulsateError>`.
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --all --check` must pass; `cargo doc` must build clean.
- Errors use the `pulsate-core` taxonomy with stable `PLS-*` codes
  ([docs/25-error-and-status-catalog.md](docs/25-error-and-status-catalog.md)).
- Data-plane crates must not depend on control-plane crates
  (`cargo xtask lint-layering`).

See [docs/03-repository.md](docs/03-repository.md) for the coding standards,
naming conventions, and testing strategy.

## Local workflow

```sh
mise run check        # fmt + clippy + build + test, or run the steps directly:
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test  --workspace
cargo xtask lint-layering
```

Commits follow [Conventional Commits](https://www.conventionalcommits.org/); the
DCO sign-off requirement is in [docs/18-open-source.md](docs/18-open-source.md).
