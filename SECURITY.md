# Security Policy

TSQL is developed with a zero-trust mindset: assume inputs are hostile, credentials are sensitive, and automation must use the least privilege necessary.

## Reporting a Vulnerability

Please do not open a public issue for a suspected vulnerability.

Use GitHub private vulnerability reporting if enabled for this repository. If it is not enabled yet, contact the repository owner directly and include:

- Affected version or commit.
- Reproduction steps.
- Impact assessment.
- Suggested mitigation, if known.

## Supported Versions

The project is pre-release. Security fixes target the latest `main` branch until the first stable release.

## Repository Protections

Recommended protections are documented in `docs/branch-protection.md`.

Required practices:

- No direct pushes to `main`.
- Pull requests for every change.
- Owner review before merge.
- Required CI and security checks.
- Manual protected release environment for crates.io publishing.

## Secret Handling

Never commit database credentials, tokens, private keys, connection strings with passwords, or production dumps.

Use environment variables or local untracked config files for secrets.
