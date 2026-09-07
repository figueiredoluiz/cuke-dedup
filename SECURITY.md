# Security policy

## Supported versions

Security fixes are released for the latest published minor version. Before the first
stable release, only the latest `0.x` release is supported.

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |
| Older versions | No |

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Use
[GitHub private vulnerability reporting](https://github.com/figueiredoluiz/cuke-dedup/security/advisories/new)
to send a private report with the affected version, impact, reproduction steps, and
any suggested mitigation. Remove credentials, proprietary source code, and unrelated
personal data from the report.

You should receive an acknowledgement within seven days. The maintainer will validate
the report, coordinate a fix and disclosure with the reporter, and publish an advisory
when users can take action. Please allow a reasonable remediation period before public
disclosure.

## Security boundaries

CukeDedup treats analyzed repositories as untrusted input. It parses source and
configuration files without executing project code. Terminal and machine-readable
report content may contain attacker-controlled text from the analyzed repository and
must not be treated as commands or agent instructions.

The GitHub Action downloads native binaries only from this repository's versioned
releases and verifies their published checksum and archive structure before execution.
Consumers should pin the Action to a complete commit SHA.

## Compromised releases

Published Cargo and npm versions and GitHub release assets are immutable. If a release
is compromised, the maintainer will publish a GitHub security advisory, deprecate the
affected npm version, yank the affected Cargo version when appropriate, and issue a
new patched version. Existing assets or version tags will not be silently replaced.
