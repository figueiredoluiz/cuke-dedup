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

Relevant definition, feature, registration-module, and baseline files are limited to
8 MiB each; configuration files are limited to 1 MiB each. Across one analysis run, registration
imports are also limited to 1,024 distinct modules, 64 MiB of aggregate module source, and 16,384
memoized path/depth states, covering every module at every supported depth. The shared resolver
confines relative imports to the canonical analysis root and reuses results across importing files.
Matcher patterns and compiled regular-expression programs are limited to
1 MiB, with a 2 MiB lazy DFA cache per matcher. Gherkin Markdown conversion collects at most
10,000 ambiguous candidates, performs at most 128 parser probes, and parses at most 64 MiB of
cumulative synthesized probe input. Candidate collection is a separate bound: the allowed probe
count and bytes can reject a smaller candidate-heavy document. Exact default-English step
candidates receive one whole-document parser probe charged by its actual synthesized size. For
dialect-dependent candidates, whole-document retry is used only when
`synthesized bytes × candidate count` is at most 64 MiB; otherwise restoration uses bounded
32-item batches. An input that exceeds a limit makes the run incomplete and exits with code `2`;
it is never silently skipped. JSON parsing retains `serde_json`'s
default recursion limit.

Definition and feature exclusions control discovery. An included definition can still cause
an explicitly imported registration module to be read for static registration resolution;
that module remains subject to the same 8 MiB per-file limit.

The GitHub Action downloads native binaries only from this repository's versioned
releases and verifies their published checksum and archive structure before execution.
Consumers should pin the Action to a complete commit SHA.

## Compromised releases

Published Cargo and npm versions and GitHub release assets are immutable. If a release
is compromised, the maintainer will publish a GitHub security advisory, deprecate the
affected npm version, yank the affected Cargo version when appropriate, and issue a
new patched version. Existing assets or version tags will not be silently replaced.
