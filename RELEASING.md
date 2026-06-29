# Releasing Pulsate

Releases are automated with [release-plz](https://release-plz.dev). It manages
versions, changelogs, and crates.io publishing across all `pulsate-*` crates in
dependency order — far more reliable than `cargo publish --workspace` once the
workspace grows.

## Pipeline

```
commits on main
      │
      ▼
release-plz  ──▶  opens a "release PR" (version bumps + CHANGELOG.md per crate)
      │
   merge PR
      │
      ▼
release-plz  ──▶  publishes every changed crate to crates.io (ordered, retried)
      │            and tags the binary crate `v{version}` + a GitHub release
      ▼
release.yml (on tag v*)  ──▶  cross-platform binaries + .deb/.rpm + GHCR image
                               attached to that release
```

Library crates publish to crates.io but get **no** per-crate tags; only the
`pulsate` binary crate is tagged `v{version}`, which is what `release.yml`,
`scripts/install.sh`, the Homebrew formula, and `cargo binstall` all key off.

## One-time setup

GitHub repo secrets:

- `CARGO_REGISTRY_TOKEN` — a crates.io API token with publish scope.
- `RELEASE_PLZ_TOKEN` *(optional)* — a fine-grained PAT (contents + PR write) so
  the release PR can trigger CI. Without it the PR still opens, but CI won't run
  on the PR itself.

The Homebrew tap (`squaretick/homebrew-tap`) holds `Formula/pulsate.rb`; bump its
`version` + four `sha256` after each release (`brew bump-formula-pr` automates it
from the release's `*.sha256` assets).

## Cutting a release

1. Land changes on `main` using [Conventional Commits](https://www.conventionalcommits.org/)
   (`feat:`, `fix:`, `feat!:` …) — release-plz derives version bumps from them.
2. Review and merge the **release PR** that release-plz opens.
3. That's it: crates.io publish, the `v{version}` tag, the GitHub release, and all
   release artifacts happen automatically.

### Local dry run

```sh
cargo install release-plz
release-plz update          # show the version bumps + changelog it would make
release-plz release-pr --dry-run
```
