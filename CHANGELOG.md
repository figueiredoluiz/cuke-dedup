# Changelog

All notable changes to CukeDedup are documented in this file. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-09-10

### Changed

- **Breaking (reports and baselines):** JSON and JSONL reports now use schema version `2`, and
  semantic baselines use version `2`. Large exact-equivalence groups are represented by bounded
  cluster evidence instead of many pair records. Existing version `1` baselines are rejected for
  normal comparison but can be replaced directly with `--update-baseline`; no manual deletion is
  required.
- **Breaking (CLI):** analysis that cannot cover the complete corpus now warns and exits on
  finding severity instead of returning operational exit code `2`. Candidate-comparison
  truncation, resolution resource limits, and registration imports that cannot be resolved
  statically are all reported as incomplete coverage rather than analyzer failure. Set
  `failOnIncomplete: true`, pass `--fail-on-incomplete`, or set the Action's
  `fail-on-incomplete` input to restore exit code `2`.
- **Breaking (SARIF):** an incomplete run now reports `executionSuccessful: false` so code-scanning
  consumers can distinguish partial coverage from a complete scan.
- **Breaking (Rust API):** `AnalysisOutcome::operational_errors` is replaced by
  `AnalysisOutcome::incomplete`. Bounded work that could not finish is a completeness signal, not
  an operational failure; `analyze` still refuses an incomplete result.
- **Breaking (Rust API):** report renderers and `write_reports` now accept a shared
  `ReportContext` instead of separate result, root, threshold, and metrics arguments. Construct it
  with `ReportContext::new` or `ReportContext::with_metrics`; the redundant
  `*_with_threshold` and `write_reports_with_metrics` wrappers were removed.

### Added

- An `overlapping-matcher` rule that reports two matchers accepting the same step text without
  needing a feature corpus to prove it.
- `registrations` project configuration plus inference for local wrappers that forward their own
  leading parameters to a known registration.
- `parameterTypes` project configuration, which restores exact usage and ambiguity analysis for
  matchers that use project-defined Cucumber Expression parameter types.
- `failOnIncomplete` project configuration, the `--fail-on-incomplete` flag, and the Action's
  `fail-on-incomplete` input.
- A `corpus.incomplete` flag in machine reports and a matching HTML alert when a registration
  import could not be resolved statically.
- Legacy Cucumber.js and Cypress Cucumber registration entrypoints, plus Playwright-BDD
  class-method decorator registrations.
- A `corpus.featureFilesWithoutSteps` census field for converted Gherkin Markdown inputs that
  yielded no concrete steps.

### Security

- Repository-controlled configuration can lower analysis candidate budgets but can no longer
  raise their immutable memory and work ceilings.
- npm publishing now uses short-lived OIDC trusted-publisher credentials exclusively; the release
  workflow no longer reads a long-lived `NPM_TOKEN` secret and fails closed on unsupported npm
  clients.
- A second fuzz target covers configuration discovery, project module metadata, and baseline
  parsing, the untrusted inputs that are read outside the source parsers.
- Release jobs are bound to the immutable workflow commit instead of accepting a free-form checkout
  ref, and the GitHub Action no longer derives outbound download URLs from runtime file contents.

### Performance

- Feature-step usage analysis evaluates every matcher in one pass with a shared regular-expression
  set and matches each distinct step text once, roughly halving usage-dominated analysis time.

### Fixed

- Analyzing a single package whose imports resolve outside it no longer fails the run; the
  unresolved specifier is reported as a source-localized warning naming its cause.
- Registration modules the filesystem refuses to read remain operational failures, matching how
  unreadable definition sources are already treated.
- Semantic baselines are no longer updated from an incomplete corpus.
- Wrapper inference no longer promotes deferred callbacks, nested declarations, generators, async
  functions, conditional calls, or multi-statement helpers into file-wide registration aliases.
- Ambiguity evidence uses linear membership storage, and static overlap work shares the configured
  candidate budget and caps retained findings instead of allocating an unbounded quadratic pair
  or finding set.
- Oversized combined matcher indexes are split into bounded regular-expression sets instead of
  silently restoring the definitions-by-steps fallback loop.
- npm release validation now covers `package-lock.json`, including the root package, platform
  workspaces, and optional dependency versions.
- Gherkin Markdown that converts to an empty feature document now marks the corpus incomplete and
  disables `unused-definition` instead of producing false non-use findings.
- Non-authoritative Cucumber Expression fallbacks and JavaScript regular expressions using the
  Unicode-sets `v` flag can no longer produce false unused or ambiguity conclusions.
- Playwright-BDD factory aliases now require trusted ESM or CommonJS import evidence, and local
  registration barrels preserve imported framework bindings across recoverable resolver errors.
- Static overlap witness scans are bounded before index evaluation, handler behavior signatures
  normalize renamed receivers, and SARIF preserves pre-truncated cluster metadata.

## [0.1.1] - 2026-09-08

### Changed

- Upgraded `tree-sitter` to 0.27.0 and `taiki-e/install-action` to 2.87.5.

### Fixed

- Added package-specific READMEs to native npm distributions.
- Adopted conventional `windows-*-msvc` names for the Windows npm packages.
- Made npm publication safe to retry after a partial release.

## [0.1.0] - 2026-09-07

Initial public release.

### Added

- Static duplicate, near-duplicate, ambiguous, reusable, and unused Cucumber step analysis.
- JavaScript, JSX, TypeScript, and TSX source adapters and Gherkin Markdown support.
- Terminal, JSON, JSON Lines, HTML, and SARIF reporters.
- Duplication thresholds, semantic baselines, changed-file analysis, suppressions, and ignore files.
- Native Cargo and npm distributions for eight supported targets.
- A checksum-verified GitHub Action and an agent-oriented CukeDedup skill.

[0.2.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/figueiredoluiz/cuke-dedup/releases/tag/v0.1.0
