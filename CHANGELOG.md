# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Per-crate changelogs are generated from [Conventional Commits][cc] by
`release-plz`; this file records workspace-level, user-facing highlights.

[cc]: https://www.conventionalcommits.org/

## [Unreleased]

### Added
- ACME account/JOSE layer (`pulsate-acme`): ECDSA P-256 (ES256) account keys,
  RFC 7638 JWK thumbprints, HTTP-01 key authorization, and flattened JWS signing
  (RFC 8555) — the cryptographic core for live certificate issuance.
- `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1).
- `SECURITY.md` vulnerability-disclosure policy.
- OpenSSF Best Practices conformance: changelog, issue/PR templates.

### Changed
- License finalized as **Apache-2.0** (patent grant + cloud-native ecosystem fit).
- Project domain moved to `squaretick.dev/pulsate`.

## [0.1.0]

### Added
- Initial public release of the Pulsate workspace (~26 `pulsate-*` crates):
  HTTP stack, reverse proxy, router, config/flow language, WAF, cache, pipeline,
  TLS, observability, plugins, dashboard, CLI, and admin control plane.

[Unreleased]: https://github.com/squaretick/pulsate/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/squaretick/pulsate/releases/tag/v0.1.0
