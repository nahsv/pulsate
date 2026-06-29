# Security Policy

We take the security of Pulsate seriously. Thank you for helping keep Pulsate
and its users safe through responsible disclosure.

## Reporting a Vulnerability

**Do not report security vulnerabilities through public GitHub issues, pull
requests, or discussions.**

Instead, report privately through either of:

- **Email:** [security@squaretick.dev](mailto:security@squaretick.dev) or [webmaster@squaretick.dev](mailto:webmaster@squaretick.dev) (PGP key available on request)
- **GitHub:** open a [private security advisory](https://github.com/squaretick/pulsate/security/advisories/new)

Please include as much of the following as you can:

- The type of issue (e.g. buffer overflow, request smuggling, auth bypass, SSRF)
- Affected component(s), version(s), and configuration
- Step-by-step reproduction, proof-of-concept, or exploit code
- Impact assessment and how an attacker might leverage it

## Response Process

- **Acknowledgement:** within **48 hours** of your report.
- **Triage:** severity assessment (CVSS) and confirmation of impact.
- **Disclosure timeline:** coordinated disclosure, target **≤ 90 days**, faster
  for actively-exploited issues.
- **Fix & release:** the patch is developed privately and shipped as an
  out-of-band security release across all supported versions, with a CVE and a
  published advisory.

## Supported Versions

Security fixes are provided for the **current** and **previous** minor release.

| Version | Supported |
| ------- | --------- |
| 0.1.x   | ✅        |

This window widens as the project matures (LTS lines are planned for the
enterprise edition).

## Recognition

We maintain a security hall-of-fame for responsibly-disclosed reports. A bug
bounty may be offered as the project matures.

## Transparency

Pulsate's hardening posture is public: see the
[Threat Model](docs/21-threat-model.md), the
[supply-chain measures](docs/33-release-engineering-and-supply-chain.md), and
the [open-source security policy](docs/18-open-source.md).
