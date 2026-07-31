# Security policy

## Supported versions

Until the first stable release, security fixes are made on the latest `main`
branch and most recent `0.x` tag only.

## Reporting

Please use GitHub's private security-advisory workflow for the
`haneoka-gakuen/deneb` repository. Include:

- affected Deneb version and target;
- a minimal project/runtime/plugin fixture;
- impact and required attacker control;
- whether the issue escapes a project, runtime, plugin, or output boundary.

Do not open a public issue before maintainers have had a reasonable opportunity
to assess a path traversal, code-execution, signing, or supply-chain report.
The architecture's enforced and non-enforced trust boundaries are documented
in [docs/security.md](docs/security.md).
