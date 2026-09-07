# CukeDedup

[![CI](https://github.com/figueiredoluiz/cuke-dedup/actions/workflows/ci.yml/badge.svg)](https://github.com/figueiredoluiz/cuke-dedup/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

CukeDedup finds duplicate, near-duplicate, ambiguous, reusable, and unused Cucumber/Gherkin step definitions. It analyzes source code statically: project configuration and test code are never executed.

Current support includes:

- JavaScript/JSX and TypeScript/TSX step definitions, including common module variants.
- Cucumber.js, Playwright BDD, and Cypress Cucumber workflows.
- Classic `.feature` files and Gherkin Markdown `.feature.md` files.
- Terminal, JSON, JSON Lines, HTML, and SARIF reports.
- Threshold, baseline, changed-file, suppression, and ignore-file workflows.

## Install

Install the native CLI through Cargo:

```sh
cargo install cuke-dedup
```

Or install the native launcher in a JavaScript project:

```sh
npm install --save-dev cuke-dedup
npx cuke-dedup .
```

To build from source, clone the repository with Rust 1.90 or newer and run
`cargo build --release --locked`.

## Usage

The path-only and explicit `check` forms are equivalent when both include a path:

```sh
cuke-dedup .
cuke-dedup check .
cuke-dedup . --threshold 5
cuke-dedup . --print-config
cuke-dedup . --reporters terminal,json,html,sarif
cuke-dedup . --reporters jsonl
cuke-dedup . --output reports/cuke-dedup
```

`check` is reserved as the explicit subcommand and requires a path. To analyze a directory literally named `check`, use `cuke-dedup ./check`.

With no configuration, CukeDedup respects `.gitignore` and `.cuke-dedupignore`, excludes common generated directories, discovers classic `.feature` files and Gherkin Markdown `.feature.md` files, and inspects conventional JavaScript/TypeScript step registrations such as `Given`, `When`, `Then`, and `defineStep`, including common aliases.

## Configuration

### Precedence and config files

Configuration precedence is:

```text
CLI flags > explicit --config > auto-discovered CukeDedup config > detected framework configuration > built-in defaults
```

CukeDedup accepts `--config/-c <file>`, auto-discovers one configuration source, and lets CLI values replace matching config values. Automatic discovery stops at the first valid source in this order:

```text
.cuke-dedup.json
.config/cuke-dedup.json
.config/.cuke-dedup.json
cuke-dedup.config.json (legacy compatibility)
package.json#cukeDedup
```

An explicit `--config` path bypasses automatic discovery and is a fatal configuration error when it cannot be read or parsed. Relative config, output, and baseline paths are resolved from the analyzed root. An invalid auto-discovered file emits a warning and falls through to the next source. JavaScript projects may use the `cukeDedup` key in `package.json`; other repositories should prefer `.cuke-dedup.json`.

### Example configuration

```json
{
  "definitions": ["features/steps/**/*.ts"],
  "features": ["features/**/*.{feature,feature.md}"],
  "exclude": ["dist/**"],
  "excludeDefaults": true,
  "includeHidden": false,
  "threshold": 5,
  "requireFeatures": true,
  "noMetrics": false,
  "reporters": ["terminal", "json", "html", "sarif"],
  "output": "reports/cuke-dedup",
  "rules": {
    "duplicate-matcher": "error",
    "duplicate-handler": "error",
    "near-duplicate-step": "warning",
    "unused-definition": "off"
  },
  "suppressions": [
    {
      "rule": "duplicate-handler",
      "path": "features/steps/legacy.ts",
      "matcher": "the legacy flow is complete",
      "reason": "Kept distinct while the legacy flow is retired"
    }
  ]
}
```

### Suppressions

Every suppression must include a non-empty `reason` and select at least a `path` or `matcher`:

- When both selectors are present, both must match.
- For a pair finding, a configured `path` must contain every involved definition.
- A matcher selector must select at least one involved definition.

These rules prevent a directory-scoped exception from hiding a conflict that crosses into maintained code.

One definition can instead carry an auditable source-local suppression. The directive must be immediately above the registration, select one rule, and include a reason:

```ts
// cuke-dedup:ignore duplicate-handler -- retained for an external compatibility contract
Given("the legacy flow completes", legacyHandler);
```

Malformed directives are operational errors rather than silently ignored comments. A directive suppresses findings involving its attached definition only.

### Framework discovery and corpus boundaries

When `features` is not explicitly configured, CukeDedup statically reads literal paths from the project's BDD setup: Playwright-BDD's `defineBddConfig`, Cypress `e2e.specPattern` when the Badeball Cucumber preprocessor is installed, and Cucumber.js configuration. It never executes project configuration. Cucumber directories include both `.feature` and `.feature.md`; Playwright-BDD directories follow that framework's `.feature` default. Explicit file or glob paths are preserved, so a project can select names such as `*.spec`. Dynamic paths produce a warning and can be made deterministic by setting `features` in CukeDedup's own configuration.

Analysis is intentionally scoped to the supplied root: every discovered definition and feature under that root belongs to one comparison corpus. In a monorepo whose packages use independent step registries, run CukeDedup once per package (for example, `cuke-dedup packages/accounts`) instead of treating the monorepo root as one suite. Framework detection selects the appropriate registration patterns but does not silently split the corpus because mixed-framework projects can deliberately share definitions.

`.feature.md` selects the Gherkin Markdown parser. Other extensions selected by a custom or framework glob are treated as classic Gherkin. This prevents ordinary Markdown files from being scanned while still allowing conventions such as `.spec`.

### Exclusions and ignore files

Configured `exclude` patterns replace custom patterns from lower-precedence layers, while built-in generated-directory exclusions remain independently enabled. An exclusion may name one file, one directory, or a glob.

- Set `excludeDefaults` to `false` to analyze an explicitly selected workspace under `node_modules`, `target`, or another protected directory.
- Set `includeHidden` to `true` to descend into dot-directories.
- Definitions, features, excludes, reporters, and suppressions replace the corresponding lower-precedence value.
- Scalar values and individual rule severities merge by precedence and rule name.

Configured `definitions`, `features`, `exclude`, and suppression-path values use `globset` syntax against root-relative paths normalized with `/`. `*` and `?` may cross `/`; use `**` when a recursive directory boundary should be obvious to readers. Character classes such as `[ab]`, brace alternatives such as `{js,ts}`, and backslash escaping are supported. These configuration globs are distinct from `.gitignore` and `.cuke-dedupignore`, which use directory-scoped gitignore semantics and support negation.

Repositories may also place a `.cuke-dedupignore` file at the analyzed root or in any descendant directory. It uses gitignore syntax: blank lines and `#` comments are ignored, `/` anchors a rule to the ignore file's directory, a trailing `/` selects directories, and `!` negates an earlier matching rule. Nested files apply only to their directory subtree. These rules are applied in addition to `.gitignore`, built-in exclusions, and configured `exclude` patterns. `--no-default-excludes` disables only the built-in generated-directory list; it does not disable either ignore file. Configured `exclude` patterns remain hard exclusions and cannot be negated from `.cuke-dedupignore`.

Example `.cuke-dedupignore`:

```gitignore
# Generated feature sources
features/generated/*
**/*.generated.ts

# Keep one reviewed generated definition
!features/generated/reviewed.generated.ts
```

### CLI overrides and discovery diagnostics

Equivalent discovery and rule overrides are available from the CLI:

```sh
cuke-dedup . \
  --config .cuke-dedup.json \
  --definitions 'features/steps/**/*.ts' \
  --features 'features/**/*.feature' \
  --exclude 'generated/legacy.ts' \
  --exclude 'vendor' \
  --exclude 'dist/**/*.ts' \
  --threshold 5 \
  --rule duplicate-matcher=warning
```

`--exclude` may be repeated or receive comma-separated patterns. Patterns are resolved relative to the analyzed root. The CLI equivalents for the discovery escape hatches are `--no-default-excludes` and `--include-hidden`. Explicit definition globs do not implicitly weaken either safety default.

Use `--explain-discovery` to print the effective pattern origin, selected parser, matching pattern, and definition inputs without changing report output. Unmatched feature patterns are warnings. If definitions exist but no feature files match, CukeDedup warns and disables `unused-definition` findings rather than presenting an incomplete corpus as proof that every definition is unused. Set `requireFeatures: true` or pass `--require-features` to make that condition an operational failure (exit code `2`).

Malformed discovered JavaScript or TypeScript remains a fail-closed operational error because partial extraction could make a duplication gate pass incorrectly. Fix the syntax, use a `.tsx` extension for JSX-bearing TypeScript, or narrow `definitions` to the actual step-definition sources.

Use `--print-config` to serialize the fully merged and validated configuration as JSON and exit without discovery. The output includes the selected CukeDedup configuration source, framework-derived feature-pattern origin, CLI overrides, effective default exclusions, rule severities, and configuration warnings.

## Rules

| Rule | Default | Meaning |
| --- | --- | --- |
| `duplicate-matcher` | Error | Definitions use the same effective matcher. |
| `normalized-matcher` | Error | Matchers become equivalent after normalization. |
| `ambiguous-step` | Error | A concrete feature step matches multiple definitions. |
| `duplicate-handler` | Error | Different matchers have the same alpha-normalized handler. |
| `near-duplicate-step` | Warning | Matcher wording is close and handler structure agrees. |
| `parameterization-candidate` | Warning | Handler structures differ primarily in literal values. |
| `unused-definition` | Warning | No discovered feature step uses the definition. |

Set a rule to `off`, `warning`, or `error` in configuration. The CLI also accepts `warn` as an alias for `warning`.

Exact handler equivalence is always reported as `duplicate-handler` when matchers are not equivalent. If the matcher wording is also close, the same retained pair may carry the advisory `near-duplicate-step` finding as additional evidence.

## Duplication threshold

`threshold` controls the percentage of discovered definitions that may participate in active, error-level duplication findings. It accepts a value from `0` through `100` and defaults to `0`.

```sh
cuke-dedup . --threshold 5
```

The numerator contains unique definition locations involved in `duplicate-matcher`, `normalized-matcher`, `duplicate-handler`, and any `near-duplicate-step` or `parameterization-candidate` rule configured as an error. A definition is counted once even when it participates in several pairs. Suppressed and baselined findings do not count. The denominator is every discovered definition, including definitions without findings.

The threshold passes when the unrounded percentage is less than or equal to the configured value. Active `ambiguous-step` and `unused-definition` errors remain independently fatal because they are correctness and usage policies rather than duplication tolerance. Empty projects have a `0%` duplication rate.

## Reports

Choose reporters according to how the result will be consumed:

| Reporter | Destination | Best for |
| --- | --- | --- |
| `terminal` | stdout | Interactive local use and concise CI logs. |
| `json` | File | Structured integration data. |
| `jsonl` | stdout | Streaming shell and agent workflows. |
| `html` | File | Human review with search, filters, and theme switching. |
| `sarif` | File | Code-scanning platforms such as GitHub. |

The `jsonl` reporter emits one compact object per finding followed by a final summary object. Because `terminal` and `jsonl` both own stdout, they cannot be selected together. JSONL can be combined with file reporters without contaminating the stream.

JSON, HTML, and SARIF reports default to `reports/cuke-dedup/` and can be redirected with `--output`. Relative output paths are resolved from the analyzed root, not the shell's current directory. File-only reporter runs print the generated report paths to stdout.

### JSON and HTML

The JSON report uses schema version `1`. It includes relative source spans, severity, suppressions, similarity scores, suggested actions, structured matcher/handler evidence, threshold calculations, input counts, and execution metrics. The self-contained HTML report presents the same result with search, severity and rule filters, a light/dark theme switch, matcher differences, and side-by-side handler snippets.

Set `noMetrics: true`, pass `--no-metrics`, or use the Action's `no-metrics: true` input to omit timing data when byte-reproducible artifacts matter.

### JSON Lines

JSONL schema version `1` is intended for streaming agent and shell consumption:

```sh
cuke-dedup . --reporters jsonl \
  | jq -c 'select(.type == "finding" and .active)'
```

Each finding record is self-contained and carries a location-independent semantic fingerprint, active/suppressed state, threshold contribution, relative source spans, suggested action, and structured evidence. Free-text fields are capped at 2,000 Unicode characters; `truncatedFields` names every shortened field.

The final `type: "summary"` record declares `recordCount` and `truncated: false`, and includes aggregate counts, threshold outcome, and execution metrics. Operational warnings and errors remain on stderr, so stdout can be parsed incrementally.

### SARIF, safety, and scale

SARIF 2.1.0 contains active findings, portable relative Unicode-aware locations, stable partial fingerprints, severity, similarity properties, suggested actions, and the threshold outcome for GitHub code scanning or another compatible consumer. Handler snippets are bounded before being embedded. HTML-visible text is escaped, bidirectional and invisible-format controls—including Unicode Tag characters—are removed, and embedded JSON characters that could close a script element are encoded.

Terminal, JSON, HTML, and SARIF output retain at most 10,000 findings, prioritizing active errors before warnings and clearly reporting truncation. Their summaries still describe the complete analysis. JSONL remains the uncapped finding stream.

## Agent Skill

CukeDedup ships a portable Agent Skill that teaches coding assistants how to run the JSONL reporter, interpret its records, choose safe step-definition remediations, and verify the result without weakening the configured quality gate.

Install it from a local checkout:

```sh
npx skills add ./skills --skill cuke-dedup
```

Install it directly from GitHub:

```sh
npx skills add figueiredoluiz/cuke-dedup --skill cuke-dedup
```

The installer can target supported coding agents or install globally; consult `npx skills add --help` for the available agent and scope flags. The skill is delivered from [`skills/cuke-dedup`](https://github.com/figueiredoluiz/cuke-dedup/blob/main/skills/cuke-dedup/SKILL.md), independently of the Cargo and npm packages. Invoke it explicitly as `$cuke-dedup` where supported, or ask the agent to analyze and safely fix duplicate Cucumber step definitions.

## GitHub Action

CukeDedup can run as a native, checksum- and provenance-verified GitHub Action without compiling Rust in the consumer repository:

```yaml
permissions:
  contents: read

steps:
  - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
    with:
      fetch-depth: 0
      persist-credentials: false
  - uses: figueiredoluiz/cuke-dedup@v0.1.0
    id: cuke-dedup
    with:
      path: .
      threshold: 5
      config: .cuke-dedup.json
      exclude: |
        generated/**
        fixtures/vendor/**
      reporters: terminal,json,html,sarif
      changed-since: ${{ github.event.pull_request.base.sha }}
      no-metrics: true
  - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7
    if: always()
    with:
      name: cuke-dedup-reports
      path: reports/cuke-dedup/
```

The Action adds JSON internally when needed so its outputs are always available: `exit-code`, `duplicate-rate`, `duplicate-definitions`, `total-definitions`, `json-report`, `html-report`, and `sarif-report`. Omitted threshold, reporter, and output inputs retain the resolved project configuration; explicitly supplied Action inputs override it. Set `reporters: jsonl` when an agent-oriented workflow should receive the stream in the step log; the Action also creates its internal JSON report for outputs. Its optional `baseline` and `fail-on-new` inputs expose the semantic new-finding gate to pull-request workflows. Inputs are passed directly to the native process as an argument array. The `version` input selects one exact compatible release. Downloads fail closed when the archive, adjacent SHA-256 checksum, provenance bundle, or exact archive contents are missing or invalid. Provenance verification uses the GitHub CLI available on GitHub-hosted runners; self-hosted runners must provide `gh` on `PATH`.

For security-sensitive workflows, pin CukeDedup to the release tag's complete commit SHA. Exit codes retain the CLI contract; use `continue-on-error` only when a later workflow step intentionally evaluates the `exit-code` output.

## Incremental CI adoption

Only report findings involving files changed from a Git revision while still comparing those files against the complete definition corpus. Paths remain correct when analyzing a repository subdirectory, and untracked files are included:

```sh
cuke-dedup . --changed-since origin/main
```

Create or refresh a compact semantic baseline from the complete current analysis:

```sh
cuke-dedup . \
  --baseline .cuke-dedup-baseline.json \
  --update-baseline
```

Subsequent runs suppress the recorded multiplicity of each semantic finding. File renames and unrelated line shifts do not make a finding new, while adding another occurrence beyond the recorded count does:

```sh
cuke-dedup . --baseline .cuke-dedup-baseline.json --fail-on-new
cuke-dedup . --baseline .cuke-dedup-baseline.json --fail-on-new 3
```

`--fail-on-new` defaults to zero when no count is supplied. `--update-baseline` and `--fail-on-new` require `--baseline`; they cannot be combined. Baseline updates also reject `--changed-since`, preventing a partial scan from erasing accepted findings outside the changed-file set. The sorted, versioned baseline records one semantic fingerprint per line with a multiplicity count for reviewable diffs.

Changed-file mode still analyzes the complete discovered corpus so a changed definition can be compared with unchanged definitions. It filters the reported findings to those touching a changed file, while summary definition counts and the duplication-threshold denominator remain the complete corpus.

An empty changed-file set emits a warning so an ignored target cannot look indistinguishable from a clean incremental run. Parse errors in unchanged files do not fail changed-file mode.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The duplication rate is within the threshold and no independent error rule failed. |
| `1` | The duplication threshold was exceeded, an active non-duplication error rule failed, or the configured new-finding allowance was exceeded. |
| `2` | Discovery, parsing, configuration, or another operational failure. |

Warnings do not produce exit code `1`.

## Compatibility and limitations

- The minimum supported Rust version is 1.90. Node.js 20 and 24 are tested for the npm launcher.
- Source adapters currently cover JavaScript and TypeScript, including JSX and common module variants. Unsupported languages are rejected rather than guessed.
- CukeDedup uses static analysis and never executes framework configuration or test code. Dynamic configuration may require explicit `features` or `definitions` patterns.
- Named handlers declared in the same source file are resolved to their bodies. Imported or unresolved handler references remain available for matcher and usage rules but are not compared by identifier text.
- Malformed JavaScript or TypeScript fails closed. Unsupported regular-expression constructs emit a warning; malformed matcher escapes are operational errors.
- One analysis root is one comparison corpus. Run independent monorepo packages separately when their step registries are unrelated.
- Exact matcher and handler equivalence groups produce a linear spanning set of findings. Fuzzy structural comparisons are bounded at two million candidates and fail closed with exit code `2`; split independent suites or narrow the root if that limit is reached.
- Before version 1.0, configuration and machine-report schemas may evolve between minor releases. Schema changes will be explicit and versioned.

CukeDedup is an independent project. It is not affiliated with or endorsed by the Cucumber project or its maintainers.

## Support

Use the repository issue templates for reproducible bugs and focused feature requests. Include a minimal sanitized fixture and remove application-specific names, credentials, and source code. Report vulnerabilities privately according to [SECURITY.md](SECURITY.md).

## Development

The minimum supported Rust version is 1.90. The repository pins and tests that toolchain and also runs its quality checks on current stable Rust.

Enable the tracked pre-commit hook once per clone and run the same compliance gate directly when needed:

```sh
git config core.hooksPath .githooks
scripts/check/check.sh
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for prerequisites, focused commands, corpus and benchmark guidance, pull-request expectations, and the project decision process. Public Rust items carry rustdoc, and CI treats missing API documentation as an error.

## Releases and verification

Release notes and native archives are published on [GitHub Releases](https://github.com/figueiredoluiz/cuke-dedup/releases). Each archive has an adjacent SHA-256 checksum, a keyless Sigstore bundle, and GitHub build provenance. Verify provenance with:

```sh
gh attestation verify <archive> \
  --repo figueiredoluiz/cuke-dedup \
  --signer-workflow figueiredoluiz/cuke-dedup/.github/workflows/release.yml
```

Cargo, npm, the Git tag, and the GitHub release use the same version. See [CHANGELOG.md](CHANGELOG.md) for release history and [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) for the runtime dependency license inventory.

## License

[MIT](LICENSE)
