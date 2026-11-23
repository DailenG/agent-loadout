# Security policy

## Supported versions

Loadout is in early development. Security fixes go into the next release
only; there are no backports.

| Version | Supported |
|---|---|
| Latest release (currently 0.2.x) | Yes |
| Older releases | No; update to the latest release |
| `main` branch | Fixes land here first |

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub's private
vulnerability reporting:
<https://github.com/DailenG/agent-loadout/security/advisories/new>
(the repository's **Security** tab, **Report a vulnerability**).

Do not open a public issue, pull request or discussion for a suspected
vulnerability.

Include what you can of:

- the Loadout version (`loadout --version`) and OS;
- the steps to reproduce, or a proof of concept;
- the impact you expect (for example a secret value reaching the store, a
  client file outside Loadout's ownership being changed, a project manifest
  applied without trust, a path escape on skill import).

Do not include real secrets or real client configuration in the report;
redact them or use throwaway values.

You can expect an acknowledgment within a week. We will keep you informed
while a fix is prepared, credit you in the advisory and the
[changelog](CHANGELOG.md) unless you prefer otherwise, and publish a GitHub
security advisory once a fixed release is out.

## Scope

In scope: the `loadout` CLI, the desktop app, the installers and packages
built from this repository, and the release and packaging workflows.

Out of scope: vulnerabilities in the AI clients Loadout configures, in MCP
servers themselves, or in third-party secret providers (1Password, the OS
keyring), unless Loadout's use of them causes the problem.

How Loadout protects secrets and client files is described in
[docs/security.md](docs/security.md).
