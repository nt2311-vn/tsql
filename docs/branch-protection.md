# Branch Protection

This repository should treat `main` as a protected release-quality branch.

## Required `main` Rules

- Require a pull request before merging.
- Require approval from the repository owner.
- Dismiss stale approvals when new commits are pushed.
- Require conversation resolution before merging.
- Require branches to be up to date before merging.
- Require signed commits if available for the owner workflow.
- Disallow direct pushes to `main`.
- Disallow force pushes.
- Disallow branch deletion.
- Restrict who can push and merge to the repository owner.

## Required Status Checks

Enable these checks as required before merge:

- `ci / format`
- `ci / clippy`
- `ci / test`
- `ci / audit`
- `security / trufflehog`
- `security / gitleaks`
- `security / semgrep`
- `security / trivy`

Do not require Snyk until `SNYK_TOKEN` is configured and the optional job is stable.

## Repository Settings

- Enable Dependabot alerts.
- Enable Dependabot security updates.
- Enable secret scanning.
- Enable push protection.
- Disable GitHub Actions from creating or approving pull requests unless explicitly needed.
- Set workflow permissions to read-only by default.
- Require manual approval for outside contributors.

## Release Environment

Create a protected environment named `crates-io-release`.

Recommended settings:

- Required reviewer: repository owner.
- Deployment branches and tags: protected version tags only.
- Secret: `CARGO_REGISTRY_TOKEN`.
- Optional secret: `SNYK_TOKEN`.

## Owner Workflow

Use this flow for all changes:

1. Update `main`.
2. Create a feature branch from `main`.
3. Commit only intended changes.
4. Push the feature branch.
5. Open a pull request.
6. Wait for required checks.
7. Review the diff.
8. Merge only after checks and owner approval.
