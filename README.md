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
  "requireDefinitions": true,
  "requireFeatures": true,
  "failOnIncomplete": false,
  "registrations": ["defineDomainStep"],
  "parameterTypes": { "colour": "red|green|amber" },
  "maxCandidateComparisons": 2000000,
  "maxStructuralClassComparisons": 250000,
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

`registrations` names local helper functions that register steps. CukeDedup already infers a
top-level synchronous wrapper whose single statement forwards its own leading parameters straight
to a known registration, such as `function step(text, handler) { Given(text, handler); }`. Nested,
async, generator, conditional, multi-statement, reordered, rewritten, or dynamically built wrappers
cannot be inferred safely; declare them here instead. Declared names are assumed to take the
matcher first and the handler second, like the built-in registrations.

`parameterTypes` declares project-defined Cucumber Expression parameter types and the regular
expression each one accepts, mirroring `defineParameterType` without executing it. Undeclared
types degrade to a permissive pattern that avoids false `unused-definition` reports but cannot
prove ambiguity or overlap; a declared type restores exact usage analysis for matchers that use
it. Each pattern is validated at startup, so a malformed one fails the run instead of silently
disabling analysis.

### Suppressions

Every suppression must include a non-empty `reason` of at most 512 Unicode characters and select at least a `path` or `matcher`:

- When both selectors are present, both must match.
- For a pair finding, a configured `path` must contain every involved definition.
- A matcher selector must select at least one involved definition.

These rules prevent a directory-scoped exception from hiding a conflict that crosses into maintained code.

One definition can instead carry an auditable source-local suppression. The directive must be immediately above the registration, select one rule, and include a reason:

```ts
// cuke-dedup:ignore duplicate-handler -- retained for an external compatibility contract
Given("the legacy flow completes", legacyHandler);
```

Malformed directives and reasons longer than 512 Unicode characters are operational errors rather than silently ignored comments. A directive suppresses findings involving its attached definition only.

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
  --require-definitions \
  --rule duplicate-matcher=warning
```

`--exclude` may be repeated or receive comma-separated patterns. Patterns are resolved relative to the analyzed root. The CLI equivalents for the discovery escape hatches are `--no-default-excludes` and `--include-hidden`. Explicit definition globs do not implicitly weaken either safety default.

Use `--explain-discovery` to print the effective pattern origin, selected parser, matching pattern, and definition inputs without changing report output. Unmatched feature patterns are warnings. A definition file warns when its AST contains unresolved call sites shaped like known step registrations; ordinary helper calls do not. CLI-generated machine reports include a deterministic `corpus` census with the number of discovered definition files, files yielding definitions, extracted definitions, discovered feature files, and successfully parsed feature files. Set `requireDefinitions: true` or pass `--require-definitions` to make an empty extracted definition corpus an operational failure (exit code `2`).

If definitions exist but no feature files match, CukeDedup warns and disables `unused-definition` findings rather than presenting an incomplete corpus as proof that every definition is unused. Set `requireFeatures: true` or pass `--require-features` to make that condition an operational failure (exit code `2`). A module specifier that cannot be resolved statically — one outside the analysis root, or beyond a resolution limit — is reported the same way: a source-localized warning naming the cause, and the run is marked incomplete rather than failed. A module the filesystem refuses to read stays an operational failure, because the analyzer could not read input it was told to read.

CukeDedup follows statically resolvable registration imports through relative modules, the nearest `tsconfig.json` or `jsconfig.json` `paths`, the nearest package's `imports`, and packages declared by the analysis root's `workspaces`. Project configs may use JSONC plus relative JSON `extends` strings or arrays up to 16 files deep. Workspace entrypoints honor `exports` before `main` and `index` fallbacks. Resolution never executes configs, package scripts, or imported modules, and it does not search arbitrary packages in `node_modules`. JavaScript config inheritance and package-based `extends` are intentionally unsupported; use a contained static JSON base config for aliases that affect step registration imports.

Malformed discovered JavaScript or TypeScript remains a fail-closed operational error because partial extraction could make a duplication gate pass incorrectly. Fix the syntax, use a `.tsx` extension for JSX-bearing TypeScript, or narrow `definitions` to the actual step-definition sources.

Files that participate in analysis are read with fixed resource limits: definition sources, feature files, imported registration modules, and baselines may each be at most 8 MiB; package, CukeDedup, framework, and static project configuration files may each be at most 1 MiB. Across one analysis run, project resolution reads at most 1,024 metadata files and 16 MiB of metadata, accepts at most 16,384 mappings, and visits at most 100,000 entries while locating declared workspaces. Registration imports may read at most 1,024 distinct modules and 64 MiB in aggregate, and may evaluate at most 16,384 memoized path/depth states—one state for every module at each of the 16 supported depths. The shared resolver confines imports and package targets to their canonical analysis/package roots and reuses results across importing files. Matcher patterns and compiled programs are limited to 1 MiB, with a 2 MiB lazy-DFA cache per matcher. Matcher sets that exceed those limits are recursively split into bounded sets instead of reverting the complete usage pass to one matcher evaluation per definition and feature step. Gherkin Markdown conversion collects at most 10,000 ambiguous candidates, performs at most 128 parser probes, and parses at most 64 MiB of cumulative synthesized probe input. The 10,000-candidate value is a collection bound, not a guarantee that every candidate-heavy document fits the lower probe limits. Default-English step bullets are filtered by exact Gherkin keywords and receive one whole-document parser probe charged by its actual synthesized size. For dialect-dependent candidates, a whole-document retry is attempted only when `synthesized bytes × candidate count` is at most 64 MiB; larger candidate sets use bounded 32-item restoration batches. Candidate comparison generation and static matcher-overlap analysis share a global budget of at most 2,000,000 unique definition comparisons, and structural candidate generation considers at most 250,000 proposals per handler class by default; configure `maxCandidateComparisons` / `--max-candidate-comparisons` and `maxStructuralClassComparisons` / `--max-structural-class-comparisons` with positive integers. Static overlap inspects at most four times its remaining unique-comparison budget in raw witness proposals, retains at most 10,000 findings, uses linear membership storage for feature-proven ambiguity groups, and reports truncation rather than constructing a quadratic finding or pair set. Fuzzy matcher blocking retains at most 4,096 distinct fixed-width trigram keys per definition and 250,000 across a run, evaluates at most 2,000,000 unique posting proposals, and inspects at most 10,000,000 combined handler events. Suppression path globs and root-relative paths are compiled or normalized once and indexed by rule. Pairwise suppression lookup and overlap suppression lookup each admit at most 100,000,000 aggregate work units, while the advisory unmatched-suppression scan has its own 10,000,000-work-unit ceiling and warns if it cannot inspect every entry. Pair verification rejects an individual matcher-edit or handler-LCS matrix above 100,000,000 estimated work units and admits at most 1,000,000,000 aggregate work units across matrices and finding-evidence scans. It evaluates matcher similarity before reserving handler-LCS work, so unrelated fuzzy candidates do not consume the more expensive handler budget. Reaching any candidate-generation or verification safety limit preserves partial findings and writes reports with `analysis.truncated: true` and a nonzero skipped count. Findings that are present remain valid, so the run warns and exits on finding severity alone. A registration import that cannot be resolved statically marks the corpus incomplete the same way, because the definitions behind it were never analyzed. Either condition sets `corpus.incomplete` or `analysis.truncated` in machine reports and reports an unsuccessful SARIF invocation, refuses `--update-baseline`, and becomes operational exit code `2` under `failOnIncomplete: true` or `--fail-on-incomplete`. Discovery exclusions keep selected definition and feature files outside the corpus, but an included definition may still cause an explicitly imported registration module to be read. Configuration and baseline inputs must be reduced below their limits.

Use `--print-config` to serialize the fully merged and validated configuration as JSON and exit without discovery. The output includes the selected CukeDedup configuration source, framework-derived feature-pattern origin, CLI overrides, effective default exclusions, rule severities, and configuration warnings.

## Rules

| Rule | Default | Meaning |
| --- | --- | --- |
| `duplicate-matcher` | Error | Definitions use the same effective matcher. |
| `normalized-matcher` | Error | Matchers become equivalent after normalization. |
| `ambiguous-step` | Error | A concrete feature step matches multiple definitions. |
| `overlapping-matcher` | Warning | Two matchers accept the same step text, even where no feature exercises it yet. |
| `duplicate-handler` | Error | Different matchers have the same alpha-normalized handler. |
| `near-duplicate-step` | Warning | Matcher wording is close and handler behavior substantially overlaps. |
| `parameterization-candidate` | Warning | Handler structures differ primarily in literal values. |
| `unused-definition` | Warning | No discovered feature step uses the definition. |

### duplicate-matcher

Triggers when two definitions have the same matcher kind, source text, and effective regular-expression flags. It does not compare handlers to decide whether the matcher collision is safe: at runtime both definitions still claim the same step. Keep one definition or make their matchers intentionally distinct. Suppress only when the framework guarantees the definitions cannot coexist in one runtime scope.

### normalized-matcher

Triggers when matcher text becomes equal after Unicode, whitespace, placeholder, and regular-expression normalization even though the original source differs. Semantically distinct matcher kinds or regular-expression flags do not trigger it. Consolidate the definitions or rewrite the matchers so their intended distinction survives normalization.

### ambiguous-step

Triggers when one concrete step from the discovered feature corpus matches multiple definitions. It does not speculate about steps absent from the corpus; that is the role of `overlapping-matcher`. Make the definitions mutually exclusive, normally by narrowing one matcher or removing the duplicate.

### overlapping-matcher

Triggers when static analysis can synthesize a concrete Cucumber Expression accepted by two definitions, even if no discovered feature currently uses it. Equivalent matchers already covered by duplicate rules and unsupported regular-expression reversal do not produce this finding. Narrow one matcher before a future scenario makes the overlap a runtime ambiguity.

### duplicate-handler

Triggers when different effective matchers use the same non-trivial handler after parameter and local-variable normalization. Empty, pending, unresolved, or otherwise non-comparable handlers are excluded. Consider replacing the definitions with one parameterized step, but retain separate definitions when the shared implementation is intentional domain vocabulary.

### near-duplicate-step

Triggers when matcher wording is close and meaningful handlers share at least 50% ordered behavior, with compatible structural or canonical call evidence. Similar prose alone and unrelated handler actions do not trigger it. Review the reported pair and consolidate only when both definitions express the same behavior.

### parameterization-candidate

Triggers when handlers preserve the same control flow and calls after literal normalization and the matcher texts are sufficiently similar. It remains pair-specific so reports retain the literal and matcher differences. Replace repeated literals with a step parameter when that produces a clearer public test vocabulary.

### unused-definition

Triggers when no successfully parsed concrete feature step matches a definition. It is disabled when the feature corpus is absent or incomplete, because that run cannot prove non-use. Remove the definition, add the missing scenario, or suppress it when external/generated features intentionally provide the usage.

Set a rule to `off`, `warning`, or `error` in configuration. The CLI also accepts `warn` as an alias for `warning`.

`overlapping-matcher` complements `ambiguous-step`: ambiguity is proven by a discovered feature step, while overlap is proven from the definitions alone by synthesizing one step text each matcher accepts. A suite with no features, a generated corpus, or an overlap nothing exercises yet therefore still reports the defect before it first fails at runtime. A pair that an enabled `ambiguous-step` rule already proved is not reported twice, and witnesses are only synthesized for Cucumber Expressions — regular expressions are matched against but never reversed into a sample. Overlap work shares `maxCandidateComparisons` with the other definition-pair rules and retains at most 10,000 findings; reaching either bound marks the analysis incomplete.

Exact handler equivalence is always reported as `duplicate-handler` when matchers are not equivalent. Near-duplicate detection also compares blocked matcher candidates whose ordered handler behavior overlaps by at least 50%. When handlers are not otherwise structurally equivalent, they must share a canonical call action, including direct and fluent forms such as `page.click()` and `page.locator(...).click()`, so generic control flow or unrelated awaited operations do not create false positives.

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

The JSON report uses schema version `2`. It includes relative source spans, severity, suppressions, similarity scores, suggested actions, structured pair or cluster evidence, threshold calculations, an extraction-completeness census including a `corpus.incomplete` flag, candidate-source counts and truncation status, input counts, and execution metrics. The self-contained HTML report presents the same result with search, severity and rule filters, a light/dark theme switch, matcher differences, side-by-side handler snippets, and a visible incomplete-analysis alert.

Set `noMetrics: true`, pass `--no-metrics`, or use the Action's `no-metrics: true` input to omit timing data when byte-reproducible artifacts matter. The deterministic top-level `corpus` census remains present.

### JSON Lines

JSONL schema version `2` is intended for streaming agent and shell consumption:

```sh
cuke-dedup . --reporters jsonl \
  | jq -c 'select(.type == "finding" and .active)'
```

Each finding record is self-contained and carries a location-independent semantic fingerprint, active/suppressed state, threshold contribution, relative source spans, suggested action, and structured evidence. Free-text fields are capped at 2,000 Unicode characters; `truncatedFields` names every shortened field.

The final `type: "summary"` record declares `recordCount` and stream-level `truncated: false`, and includes aggregate counts, threshold outcome, the deterministic `analysis` status, and optional execution metrics. Operational warnings and errors remain on stderr, so stdout can be parsed incrementally.

### SARIF, safety, and scale

SARIF 2.1.0 contains active findings, portable relative Unicode-aware locations, stable partial fingerprints, severity, similarity properties, suggested actions, and the threshold outcome for GitHub code scanning or another compatible consumer. Handler snippets are bounded before being embedded. HTML-visible text is escaped, bidirectional and invisible-format controls—including Unicode Tag characters—are removed, and embedded JSON characters that could close a script element are encoded.

Terminal, JSON, HTML, and SARIF output retain at most 10,000 findings, prioritizing active errors before warnings and clearly reporting truncation. A single finding retains at most 256 cluster members in any report; `memberCount`, `membersTruncated`, and `relatedLocationsTruncated` preserve the full count and make omissions explicit. Their summaries and duplication thresholds still describe the complete analysis. JSONL remains uncapped by finding count while applying the same per-record membership bound.

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
  - uses: figueiredoluiz/cuke-dedup@v0.2.0
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

The Action adds JSON internally when needed so its outputs are always available: `exit-code`, `duplicate-rate`, `duplicate-definitions`, `total-definitions`, `json-report`, `html-report`, and `sarif-report`. Omitted threshold, reporter, and output inputs retain the resolved project configuration; explicitly supplied Action inputs override it. Set `reporters: jsonl` when an agent-oriented workflow should receive the stream in the step log; the Action also creates its internal JSON report for outputs. Its optional `baseline` and `fail-on-new` inputs expose the semantic new-finding gate to pull-request workflows, and `fail-on-incomplete: true` makes an incomplete corpus or a truncated comparison set an operational failure. Inputs are passed directly to the native process as an argument array. The `version` input selects one exact compatible release. Downloads fail closed when the archive, adjacent SHA-256 checksum, provenance bundle, or exact archive contents are missing or invalid. Provenance verification uses the downloaded bundle and the GitHub CLI available on GitHub-hosted runners, so it does not require repository secrets or expanded workflow permissions; self-hosted runners must provide `gh` on `PATH`.

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

`--fail-on-new` defaults to zero when no count is supplied. `--update-baseline` and `--fail-on-new` require `--baseline`; they cannot be combined. Baseline updates reject `--changed-since` and are skipped whenever analysis has an operational error, preventing a partial scan from erasing accepted findings. The sorted, versioned baseline records one semantic fingerprint per line with a multiplicity count for reviewable diffs. Version `1` baselines from CukeDedup 0.1 can be replaced in place by running the version `2` tool with `--update-baseline`; normal comparison rejects mismatched schemas.

Changed-file mode still analyzes the complete discovered corpus so a changed definition can be compared with unchanged definitions. It filters the reported findings to those touching a changed file, while summary definition counts and the duplication-threshold denominator remain the complete corpus.

An empty changed-file set emits a warning so an ignored target cannot look indistinguishable from a clean incremental run. Definition read or extraction failures and feature read or parse failures remain fatal even when the affected file is unchanged, because those files can still determine findings involving a changed definition.

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
- Exact matcher and handler equivalence groups of up to four definitions produce a linear spanning set of pair findings. Larger exact-equivalence groups produce one cluster finding with a bounded location and fingerprint preview, the complete member count, and an explicit truncation signal; duplication thresholds still count every member. Fuzzy and structural-similarity rules remain pair findings so their individual scores and comparisons are never hidden. Fuzzy matcher discovery uses collision-free fixed-width keys for case-folded character trigrams with digit runs normalized. Every unsaturated posting list is considered; lists above 256 definitions use a deterministic lexical-neighbor fallback instead of quadratic expansion. Candidates must also be capable of reaching the 50% handler-behavior gate and, unless otherwise structurally equivalent, share a canonical call action. The fallback preserves a linear sample for highly repetitive vocabularies but, like other bounded fuzzy indexes, does not promise every possible pair. Explicit shingle, unique-proposal, per-matrix, aggregate-verification, global-candidate, and per-structural-class limits prevent adversarial input from causing unbounded work. Reaching a work or candidate limit preserves the findings evaluated so far and explicitly marks every machine report incomplete, so the absence of a finding proves nothing while the findings present remain valid. The run warns rather than failing; set `failOnIncomplete: true`, pass `--fail-on-incomplete`, or set the Action's `fail-on-incomplete` input to make it exit `2`, or split independent suites, narrow the root, or adjust a configurable candidate limit when its report census identifies that limit as the cause.
- Before version 1.0, configuration and machine-report schemas may evolve between minor releases. Schema changes will be explicit and versioned.

CukeDedup is an independent project. It is not affiliated with or endorsed by the Cucumber project or its maintainers.

## Support

Use the repository issue templates for reproducible bugs and focused feature requests. Include a minimal sanitized fixture and remove application-specific names, credentials, and source code. Report vulnerabilities privately according to [SECURITY.md](SECURITY.md).

## Development

The minimum supported Rust version is 1.90. The repository pins and tests that toolchain and also runs its quality checks on current stable Rust.

Enable the tracked hooks once per clone. Pre-commit runs the quick development checks; pre-push requires a clean checkout at the pushed commit and runs the complete release-grade gate. Run either directly when needed:

```sh
git config core.hooksPath .githooks
scripts/check/check.sh --quick
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
