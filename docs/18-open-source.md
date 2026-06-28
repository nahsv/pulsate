# 18. Open Source

> How Pulsate is run as a project: the license and why, the governance model, the code of conduct, the contribution and RFC processes, issue/PR templates, and the security policy.

**Contents**
- [License recommendation](#license-recommendation)
- [Why Apache-2.0 (and the open-core boundary)](#why-apache-20-and-the-open-core-boundary)
- [Governance](#governance)
- [Code of conduct](#code-of-conduct)
- [Contribution process](#contribution-process)
- [RFC process](#rfc-process)
- [Issue & PR templates](#issue--pr-templates)
- [Security policy](#security-policy)
- [Trademark & branding](#trademark--branding)
- [Cross-references](#cross-references)

---

## License recommendation

**Recommended: Apache License 2.0** for the entire open-source core (all crates in the workspace, [03. Repository](03-repository.md)).

Rationale:
- **Permissive adoption.** Apache-2.0 lets companies use, embed, and ship Pulsate without copyleft friction — essential for an infrastructure component people put in their critical path.
- **Explicit patent grant.** Unlike MIT/BSD, Apache-2.0 includes a patent license and a patent-retaliation clause, which matters for an infra project and reassures enterprise legal review.
- **Ecosystem fit.** The Rust ecosystem and the proxy/cloud-native space (Envoy, Linkerd, containerd, Kubernetes) are predominantly Apache-2.0; dependency and contributor expectations align.
- **DCO over CLA.** Contributions are accepted under the **Developer Certificate of Origin** (a `Signed-off-by` line), not a CLA — lower friction, no copyright assignment, contributors keep their rights. (A future commercial edition's separate code is governed separately — see below.)

Alternatives considered and rejected: **MIT/BSD** (no patent grant), **MPL-2.0** (file-level copyleft adds friction for an embeddable lib), **AGPL** (chills commercial/enterprise adoption of an infra proxy), and **BSL/SSPL for the core** (not OSI-open; reserved instead for the *separate* future enterprise edition).

## Why Apache-2.0 (and the open-core boundary)

Pulsate is **open-core**, and the boundary is a stated promise: the OSS core is genuinely production-grade and complete on its own — auto-TLS, caching, WAF, LB, observability, plugins, clustering, dashboard are all open ([01. Vision](01-vision.md)). Commercial value comes from **additive** capabilities (a multi-region cloud control plane, advanced enterprise governance, support/SLAs — [20. Future](20-future.md)), licensed separately (likely BSL with a time-delayed open-source conversion, or commercial). We will **never** move an existing open feature behind the paywall ("no rug-pulls"); the open/closed line is documented and stable so the community can trust it.

## Governance

- **Start:** a **BDFL-lite / core-maintainer** model. A small founding maintainer team owns direction and merges, operating transparently (public roadmap, RFCs, meeting notes).
- **Evolve:** as the community grows, transition to a **steering committee** with documented membership criteria, plus **area maintainers** owning subsystems (data plane, TLS/ACME, plugins, dashboard, docs) who can review/merge in their area.
- **Decision-making:** lazy consensus for routine changes; RFC + maintainer vote for substantial ones; a documented tie-break. All decisions are public.
- **Maintainer ladder:** clear path contributor → reviewer → area maintainer → steering, with criteria and a path to step down gracefully (emeritus).
- **Foundation path:** a long-term option to donate the project to a neutral foundation (e.g., CNCF) for governance neutrality once it reaches scale — an explicit non-goal to keep it founder-captured.

## Code of conduct

- Adopt the **Contributor Covenant** (current version), enforced by a named Code of Conduct committee with a documented, confidential reporting and escalation path and a tiered enforcement ladder. The CoC applies to all project spaces (repo, chat, forums, events). A welcoming, harassment-free community is a hard requirement, not a nicety.

## Contribution process

Documented in `CONTRIBUTING.md` ([17. Documentation](17-documentation.md)):
1. **Find/oc an issue** (good-first-issue labels for newcomers) or open an RFC for big changes.
2. **Sign-off (DCO):** every commit carries `Signed-off-by:`.
3. **Branch & PR** against `main`; small, focused PRs preferred; Conventional Commits for messages (feeds changelog/versioning — [03. Repository](03-repository.md)).
4. **CI gates:** fmt, clippy, deny, tests, doc build, MSRV — all green; new behavior needs tests; new config/CLI/metrics/errors need their generated-reference entries.
5. **Review:** at least one area maintainer; constructive, timely review is a maintainer commitment.
6. **Merge:** squash with a clean message; the contributor is credited.
A `MAINTAINERS.md` lists owners per area, and an `AUTHORS`/all-contributors record recognizes every contribution (code, docs, triage, design).

## RFC process

For substantial changes (new subsystems, config-format changes, ABI changes, security-sensitive features):
1. **Open an RFC PR** to the `rfcs/` directory using the template (motivation, design, alternatives, drawbacks, security/perf impact, migration).
2. **Discussion** period (public, time-boxed) — anyone can comment; the author iterates.
3. **Final comment period** announced by a maintainer; lazy consensus unless objections.
4. **Disposition:** accepted (assigned a number, tracked to implementation), postponed, or declined (with rationale recorded — declined RFCs are valuable history).
5. **Implementation** references the RFC; the RFC is updated to "implemented" with links.
This keeps big decisions transparent and prevents bikeshedding in code review. ADRs ([24](24-architecture-decision-records.md)) capture the outcomes for posterity.

## Issue & PR templates

Provided in `.github/` (or platform equivalent):
- **Bug report:** version (`p8 version`), platform, minimal `pulsate.flow` repro, expected vs actual, logs (with redaction reminder), `p8 doctor` output.
- **Feature request:** problem, proposed solution, alternatives, whether it needs an RFC.
- **Security:** *not* a public template — routes to the security policy below.
- **PR template:** description, linked issue/RFC, type (feat/fix/docs), checklist (tests, docs, DCO, breaking-change note, changelog entry).
Labels and a triage rotation keep the tracker healthy; stale-bot is used gently with clear policies.

## Security policy

A published `SECURITY.md`:
- **Private disclosure:** report vulnerabilities to `security@nahsv.com` (or `webmaster@nahsv.com`, and/or a GitHub private advisory), **never** a public issue. PGP key provided.
- **Response SLA:** acknowledge within 48 hours; triage and severity (CVSS) assessment; coordinated-disclosure timeline (target ≤90 days, faster for actively-exploited).
- **Fix & release:** patch developed privately, released across supported versions as an out-of-band security release ([03. Repository](03-repository.md)), with a CVE and an advisory.
- **Supported versions:** a documented support window (current + previous minor; LTS lines in the enterprise edition).
- **Recognition:** a security hall-of-fame / optional bug-bounty as the project matures.
- **Hardening transparency:** the [21. Threat Model](21-threat-model.md), supply-chain measures ([33](33-release-engineering-and-supply-chain.md)), and crypto choices are public.

## Trademark & branding

- The **Pulsate name and logo** are protected (trademark) even though the code is permissively licensed — so forks can use the code but not impersonate the project. A clear trademark policy permits nominative use ("works with Pulsate") while protecting the identity, following the model of other OSS infra projects.

## Cross-references
- [03. Repository](03-repository.md) — Conventional Commits, CI gates, release/versioning.
- [17. Documentation](17-documentation.md) — CONTRIBUTING and contributor docs.
- [20. Future](20-future.md) — the enterprise edition and open-core boundary.
- [21. Threat Model](21-threat-model.md) & [33. Release Engineering](33-release-engineering-and-supply-chain.md) — security transparency.
- [24. ADRs](24-architecture-decision-records.md) — recorded outcomes of RFCs.
