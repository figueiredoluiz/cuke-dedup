# CukeDedup

[![CI](https://github.com/figueiredoluiz/cuke-dedup/actions/workflows/ci.yml/badge.svg)](https://github.com/figueiredoluiz/cuke-dedup/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

CukeDedup is a fast static analyzer for duplicate, ambiguous, reusable, and unused
Cucumber/Gherkin step definitions. It reads project configuration and test code without executing
either one.

It supports:

- JavaScript, JSX, TypeScript, and TSX step definitions.
- Cucumber.js, Playwright-BDD, and Cypress Cucumber projects.
- Classic `.feature` files and Gherkin Markdown `.feature.md` files.
- Terminal, JSON, JSON Lines, HTML, and SARIF reports.
- Thresholds, baselines, changed-file analysis, suppressions, and ignore files.

## Install

With Cargo:

```sh
cargo install cuke-dedup
```

Or in a JavaScript project:

```sh
npm install --save-dev cuke-dedup
npx cuke-dedup .
```

Building from source requires Rust 1.90 or newer:

```sh
cargo build --release --locked
```

## Quick start

Run CukeDedup from the root of a test project:

```sh
cuke-dedup .
```

No configuration is required. CukeDedup respects `.gitignore` and `.cuke-dedupignore`, skips
common generated directories, discovers conventional Gherkin and JavaScript/TypeScript files, and
recognizes common `Given`, `When`, `Then`, and `defineStep` registrations and aliases.

Useful commands:

```sh
cuke-dedup . --threshold 5
cuke-dedup . --print-config
cuke-dedup . --reporters terminal,json,html,sarif
cuke-dedup . --reporters jsonl
cuke-dedup . --output reports/cuke-dedup
```

`cuke-dedup check .` is equivalent to `cuke-dedup .`. The explicit `check` form requires a path;
use `cuke-dedup ./check` to analyze a directory literally named `check`.

## Framework support

| Workflow | Recognized registrations |
| --- | --- |
| Cucumber.js | `@cucumber/cucumber` and legacy `cucumber`; ESM, CJS, aliases, namespaces, and static local re-exports. |
| Playwright-BDD | `createBdd()` registrations and `playwright-bdd/decorators` class-method decorators. |
| Cypress Cucumber | `@badeball/cypress-cucumber-preprocessor` and legacy `cypress-cucumber-preprocessor/steps`. |

Package entrypoints are matched exactly. Plain Playwright projects are supported when their
Gherkin bindings use Cucumber.js or Playwright-BDD.

See [Discovery and frameworks](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/discovery-and-frameworks.md)
for configuration detection, monorepo boundaries, module resolution, and incomplete-corpus
behavior.

## Configuration

CukeDedup reads an explicit `--config` file, an auto-discovered CukeDedup configuration, or
supported framework configuration. CLI flags always have the highest precedence.

A minimal `.cuke-dedup.json` might be:

```json
{
  "definitions": ["features/steps/**/*.ts"],
  "features": ["features/**/*.{feature,feature.md}"],
  "exclude": ["dist/**"],
  "threshold": 5,
  "reporters": ["terminal", "html"]
}
```

Use `.cuke-dedupignore` for repository-specific exclusions:

```gitignore
features/generated/*
**/*.generated.ts
!features/generated/reviewed.generated.ts
```

See the [configuration reference](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/configuration.md)
for precedence, every setting, glob semantics, suppressions, and discovery diagnostics.

## Findings and reports

CukeDedup detects eight classes of duplication, ambiguity, reuse, and unused definitions. Exact
collisions default to errors; heuristic findings default to warnings. See the
[rule reference](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/rules.md) for the
meaning, default severity, and remediation guidance for every rule.

| Reporter | Best for |
| --- | --- |
| `terminal` | Interactive use and concise CI logs. |
| `json` | Structured integrations and complete result metadata. |
| `jsonl` | Streaming shell and agent workflows. |
| `html` | Searchable human review with light and dark themes. |
| `sarif` | GitHub code scanning and compatible platforms. |

JSON, HTML, and SARIF files default to `reports/cuke-dedup/`. See
[Reports](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/reports.md) for schemas,
destinations, reproducibility, truncation signals, and reporter-specific behavior.

## GitHub Action

The Action downloads a native binary and verifies its checksum and build provenance:

```yaml
permissions:
  contents: read

steps:
  - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
    with:
      fetch-depth: 0
      persist-credentials: false
  - uses: figueiredoluiz/cuke-dedup@v0.2.0
    with:
      path: .
      threshold: 5
      reporters: terminal,json,html,sarif
```

For changed-file checks, baselines, report artifacts, outputs, and hardened pinning, see
[CI and baselines](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/ci-and-baselines.md).

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The configured quality gate passed. |
| `1` | The duplication threshold, an error rule, or the new-finding allowance failed. |
| `2` | Discovery, parsing, configuration, or another operational error prevented a valid run. |

Warnings alone do not produce exit code `1`.

## Agent Skill

CukeDedup includes an Agent Skill for safely interpreting and remediating JSONL findings:

```sh
npx skills add figueiredoluiz/cuke-dedup --skill cuke-dedup
```

From a local checkout, use `npx skills add ./skills --skill cuke-dedup`.

The installer can target supported coding agents or install globally. Run
`npx skills add --help` for agent and scope options.

Invoke it as `$cuke-dedup` where supported, or ask the agent to analyze and safely fix duplicate
Cucumber step definitions. The skill lives in
[`skills/cuke-dedup`](https://github.com/figueiredoluiz/cuke-dedup/blob/main/skills/cuke-dedup/SKILL.md)
and is distributed independently from the Cargo and npm packages.

## Documentation

- [Configuration](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/configuration.md)
- [Discovery and frameworks](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/discovery-and-frameworks.md)
- [Rules and duplication threshold](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/rules.md)
- [Reports](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/reports.md)
- [CI and baselines](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/ci-and-baselines.md)
- [Safety and limitations](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/safety-and-limitations.md)

## Compatibility

- Rust 1.90 or newer is required. The npm launcher is tested on Node.js 20 and 24.
- Definition extraction currently supports JavaScript and TypeScript, including JSX and common
  module variants.
- Static analysis cannot safely resolve every dynamic configuration, matcher, wrapper, or imported
  handler. CukeDedup reports incomplete analysis instead of treating missing evidence as clean.
- Before version 1.0, configuration and machine-report schemas may change between minor releases.

See [Safety and limitations](https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/safety-and-limitations.md)
for the complete compatibility contract and resource limits.

## Contributing and support

See [CONTRIBUTING.md](CONTRIBUTING.md) for development commands, corpus guidance, and pull-request
expectations. Use the issue templates for reproducible bugs and focused feature requests. Report
vulnerabilities privately according to [SECURITY.md](SECURITY.md). Bug reports should include a
minimal sanitized fixture with application-specific names, credentials, and source removed.

CukeDedup is an independent project. It is not affiliated with or endorsed by the Cucumber project
or its maintainers.

## Releases

Release archives include SHA-256 checksums, keyless Sigstore bundles, and GitHub build provenance.
See [GitHub Releases](https://github.com/figueiredoluiz/cuke-dedup/releases) and
[CHANGELOG.md](CHANGELOG.md) for published versions and release notes. Runtime dependency licenses
are listed in [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).

```sh
gh attestation verify <archive> \
  --repo figueiredoluiz/cuke-dedup \
  --signer-workflow figueiredoluiz/cuke-dedup/.github/workflows/release.yml
```

## License

[MIT](LICENSE)
