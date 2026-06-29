# 33. Release Engineering and Supply Chain

> How a Pulsate release is built, proven, signed, and shipped so that what you run is exactly what was reviewed: reproducible builds, SBOMs, artifact signing and provenance (SLSA), dependency policy, release channels, distribution, and update verification.

**Contents**
- [Why this matters](#why-this-matters)
- [Reproducible builds](#reproducible-builds)
- [SBOM generation](#sbom-generation)
- [Artifact signing & provenance](#artifact-signing--provenance)
- [Dependency policy & vetting](#dependency-policy--vetting)
- [Release channels & versioning](#release-channels--versioning)
- [Distribution](#distribution)
- [Update verification](#update-verification)
- [Plugin supply chain](#plugin-supply-chain)
- [Cross-references](#cross-references)

---

## Why this matters

Pulsate sits in the critical path of others' traffic; a compromised build is catastrophic. Supply-chain integrity is therefore a first-class requirement, not an afterthought — and it backs the trust claims in the [21. Threat Model](21-threat-model.md). The goal: a user (or auditor) can verify that a downloaded binary corresponds to reviewed source, built by our pipeline, with a known dependency set.

## Reproducible builds

- **Deterministic output:** pinned toolchain (`rust-toolchain.toml`), `Cargo.lock` committed, vendored or hash-locked dependencies, and build flags that eliminate nondeterminism (no embedded timestamps/paths; `--remap-path-prefix`; sorted inputs).
- **Verifiable:** the release pipeline records the exact inputs; a third party rebuilding from the tagged source + lockfile gets a byte-identical (or content-identical) artifact. We publish a rebuild guide and run an independent rebuild as a release gate.
- **Hermetic CI:** builds run in pinned, network-restricted containers so the build environment is itself reproducible.

## SBOM generation

- A **CycloneDX SBOM** is generated for every artifact (from `cargo`/`cargo-cyclonedx`), enumerating every crate, version, and license — including transitive deps and the frontend bundle.
- SBOMs ship **alongside** each release artifact and are signed.
- They feed automated vulnerability scanning (continuous: a new advisory against a shipped dependency triggers an alert and, if warranted, a patch release per the [security policy](18-open-source.md)).

## Artifact signing & provenance

- **Signing:** every binary, container image, package, and SBOM is signed with **Sigstore/cosign** (keyless, transparency-logged via Rekor) — anyone can verify authenticity without managing our public keys out-of-band.
- **Provenance (SLSA):** the pipeline emits **SLSA** build provenance attesting *what* was built, *from which source commit*, *by which builder* — targeting a high SLSA level (build L3: hardened, non-falsifiable provenance).
- **Transparency:** signatures and attestations are logged publicly; release artifacts carry checksums (SHA-256) plus the cosign bundle.
- **Container images** get the same treatment (signed, with provenance and SBOM attached as OCI referrers); base images are distroless and pinned by digest.

## Dependency policy & vetting

- **`cargo-deny`** gates every build: an allow-list of licenses (Apache-2.0-compatible), a ban-list, advisory-database checks (RUSTSEC), and duplicate-version limits ([03. Repository](03-repository.md)).
- **`cargo-audit`** runs in CI and continuously against the lockfile.
- **`cargo-vet`/supply-chain review:** new or updated dependencies require a documented review (who audited, why trusted); we prefer well-maintained, widely-used crates and minimize the dependency surface (a stated value — fewer deps, less risk).
- **`unsafe` budget:** dependencies (and our own crates) with `unsafe` get extra scrutiny; `cargo-geiger` tracks the `unsafe` footprint over time.
- **Pinning & updates:** `Cargo.lock` is committed; dependency bumps go through CI (incl. `minimal-versions` and MSRV checks) and are batched/reviewed, not auto-merged blindly.

## Release channels & versioning

(Builds on [03. Repository — Release & Versioning](03-repository.md).)
| Channel | Source | Signing | Audience |
|---|---|---|---|
| `nightly` | every `main` commit | signed, marked pre-release | testers, CI |
| `beta` | release-candidate branch | signed | early adopters, hardening |
| `stable` | tagged release | signed + provenance + SBOM | production |
| `lts` (enterprise) | long-term branch | signed + backports | regulated/large fleets ([20. Future](20-future.md)) |

SemVer for the binary/public crates; independent `flow_version` and plugin-ABI versions ([03. Repository](03-repository.md#versioning)). Release notes are generated from Conventional Commits with curated highlights and an explicit breaking-changes/migration section.

## Distribution

- **Channels:** static binaries (per OS/arch), distroless container images (multi-arch, digest-pinned), OS packages (`.deb`/`.rpm` from signed repos), Homebrew/Scoop, and `cargo binstall`.
- **Mirrors & registries:** images on major registries; binaries on a CDN with checksums/signatures; an `install.sh` that verifies signatures before placing the binary.
- **Air-gapped:** an offline bundle (binary + SBOM + signatures + offline plugin registry) for environments without internet ([20. Future](20-future.md) enterprise).

## Update verification

- **`pulsate upgrade`** ([13. CLI](13-cli.md)) fetches the target release, **verifies the cosign signature and SLSA provenance and the SBOM** against the expected identity *before* swapping the binary, and supports zero-downtime handoff. A failed verification aborts the upgrade.
- **`pulsate version --json`** reports the build's commit, provenance reference, and supported `flow_version`/plugin-ABI ranges so fleets can audit what's running.
- **Rollback** keeps the prior verified binary for instant revert.

## Plugin supply chain

Plugins are third-party code, so they get supply-chain controls too ([12. Plugins](12-plugins.md), [21. Threat Model](21-threat-model.md)):
- **Signing & verification:** plugins can be signed (cosign); `plugins { require_signed true; trusted_keys [...] }` makes Pulsate refuse unsigned/untrusted `.wasm`.
- **Provenance & SBOM** for marketplace plugins; **capability transparency** (the plugin's requested capabilities are shown before install).
- **Distribution via OCI** (`source "oci://..."`) reuses container registry signing/provenance infrastructure.
- **Pinning:** plugins are referenced by version/digest, not a floating tag, so a registry compromise can't silently swap them.

## Cross-references
- [03. Repository](03-repository.md) — CI/CD pipeline, channels, versioning, `cargo-deny`.
- [21. Threat Model](21-threat-model.md) — supply-chain threats this mitigates.
- [18. Open Source](18-open-source.md) — security policy & coordinated disclosure.
- [12. Plugins](12-plugins.md) — plugin signing/capability model.
- [13. CLI](13-cli.md) — `pulsate upgrade`/`version` verification.
