# Changelog

All notable changes to CukeDedup are documented in this file. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0] - 2026-09-24

### Changed

- A discovered source file that cannot be parsed no longer aborts the whole scan. By default its
  error-recovered definitions are still analyzed and the parse failure is reported as a completeness
  warning, so a single malformed file does not stop the run. The new `--fail-on-unparseable`
  (config `failOnUnparseable`) restores a hard stop for exactly that case, and the existing
  `--fail-on-incomplete` still rejects it as one kind of corpus incompleteness.
- `--baseline-from-ref` now tolerates an incomplete baseline revision by default instead of refusing
  to run. When the baseline excludes a generated bundle, contains an unparseable file, or hits a
  work limit, the comparison proceeds against the findings it did extract and warns that a finding
  the baseline could not extract may appear as new; `--fail-on-incomplete` restores the rejection. A
  baseline that produced a hard error, discovered definition files yet extracted no definitions, or
  has definitions but no discovered feature files, still fails regardless.
- Generated, compressed, and minified sources are excluded before parsing instead of being analyzed
  and discarded. A binary file, an archive or compression signature, or minified line geometry no
  longer contributes definitions; each exclusion is reported as a warning naming the file and its
  reason. Because the analyzed corpus then no longer represents everything discovered, the run is
  marked incomplete so `--fail-on-incomplete` fails closed rather than letting a duplicate hidden in
  an excluded bundle pass the gate.

### Fixed

- Regex matchers that differ only by effective or reordered flags now share one matcher identity, so
  a pair such as `/step/gi` and `/step/ig`, or two matchers whose only difference is a `g`, `y`, or
  `d` flag that does not change what text they match, is reported as `duplicate-matcher` instead of
  being missed.
- CommonJS barrel modules that re-export step registrations through `module.exports` — an object of
  local bindings, `module.exports = require('./steps')`, an object spread, `Object.assign`, or
  `exports.x = …` — now resolve, so definitions behind them are analyzed instead of silently
  missing. An export form the analyzer still cannot model marks the corpus incomplete rather than
  resolving to nothing, so a hidden registration is reported rather than passing as a clean run.
- A module constant declared *after* the registrations that read it now resolves. A handler is a
  deferred callback, so by the time it runs the module has finished initializing; declaration order
  was a temporal-dead-zone rule wrongly shared with the synchronous handler body. Two handlers that
  read the same later-declared value are now recognized as duplicates, so affected reports gain
  findings.
- Extraction of large generated bundles no longer takes minutes. Resolving each assignment to its
  lexical scope followed the parse tree's ancestors, which tree-sitter answers by restarting from
  the root, so a deeply nested minified file cost time quadratic in its nesting; and alias
  propagation raised its property depth without bound when the alias graph closed a cycle, which
  such files do. Scopes now resolve through a precomputed chain and propagation depth saturates at
  the only threshold that affects the result. Findings are unchanged — three real bundles that did
  not finish within minutes now extract in seconds with identical output.

## [0.7.0] - 2026-09-21

### Changed

- **Breaking (reports and baselines):** semantic fingerprints now use the first 128 bits of SHA-256 instead of FNV-1a-64. JSON and JSONL reports use schema version `3`, SARIF publishes `cukeDedupFingerprint/v3`, and semantic baselines use version `3`. Existing version `1` or `2` baselines are rejected for comparison and can be regenerated in place with `--update-baseline`.
- Matcher compilation now runs in bounded windows instead of retaining every compiled matcher at once, substantially reducing peak memory on large definition corpora. Complete, untruncated runs retain the same findings; a run that exhausts its matcher-scan budget may become incomplete or retain a different bounded subset than v0.6.0.

## [0.6.0] - 2026-09-18

### Added

- `assertionModules` names module specifiers whose `expect` export is a trusted assertion factory.
  Use it for a local module that re-exports `expect`, or for a runner this build does not recognise
  by name. Both `import` and `require` spellings honour the setting, including
  `import * as fixtures from "./fixtures"` and `import fixtures = require("./fixtures")`. An
  undeclared module stays untrusted, and a facade inferred from registration re-exports is not
  trusted for assertions.

### Changed

- `vitest`, `chai` and `bun:test` join the recognised assertion factories. Importing `expect` from
  one of them was previously *less* trusted than relying on an injected global, so explicit imports
  were penalised: expected values did not count as behaviour and two steps asserting different
  values could be offered as a `parameterization-candidate`. Chain recognition is shape-based, so
  Chai's `expect(x).to.equal(y)` is read exactly as `expect(x).toBe(y)` is. **Reports can lose those
  spurious findings on upgrade.**
- A module-scoped constant now reaches the handler fingerprint, so two handlers reading the same
  value through differently named constants are recognised as the same handler. A mutable `let`
  stays unproven, a handler-local binding of the same name shadows the module constant, and a
  constant declared inside another declaration's initializer is out of scope and never substituted.
  A name captured from an enclosing scope is nearer than the module's, so a module constant never
  substitutes over it. **Reports can gain findings on upgrade.**

### Fixed

- A function declared in a handler and then called contributes its assertions to that handler, so
  conflicting expected values inside it keep the handlers apart instead of surfacing as a
  `parameterization-candidate`. Expansion is single level and terminates on recursion. It is
  declined when the name is bound by anything else in the handler — another declaration, a variable,
  a parameter, a catch binding, a `class`, an `enum`, or a reassignment — because a name-keyed lookup
  cannot tell which body a shadowed call reaches, and expanding the wrong one would attribute
  assertions the handler never runs. A transparent wrapper such as `(check)()` resolves the same
  declaration a bare call does.

### Known limitations

- A module-scoped constant reaches the handler fingerprint only when it is declared **before** the
  registrations that read it. Two handlers reading the same value through constants declared after
  the `Given`/`When`/`Then` calls are not recognised as the same handler, so the duplicate is
  missed. Moving the declarations above the registrations restores the finding.
- A handler passed as a call expression, such as `Given("...", makeHandler())`, cannot be resolved
  to a body and takes no part in the handler rules; it is reported as `dynamic or unsupported step
  handler cannot be compared statically`. A handler reached through a member expression, such as
  `Given("...", steps.run)`, is not compared either, and that case is silent. Inline functions —
  arrow, `async`, `function` and generator — and a reference to a local function declaration are
  all compared normally.
- Step definitions registered through a default import or TypeScript's `import x = require(...)`
  and called as a member, such as `cucumber.Given("...")`, are not discovered, so those files
  contribute no definitions to any rule. Named imports, namespace imports (`import * as cucumber`),
  renamed named imports and `require` destructuring are all recognised.

## [0.5.0] - 2026-09-16

### Added

- Interactive terminal reports show the tool version, selective color, a finding-count footer, and detection time. `NO_COLOR` disables ANSI styling; redirected output remains plain text.

### Changed

- Terminal findings have a blank line after each rule heading and between findings instead of divider lines.
- `obj['name']` and `obj.name` read the same property, so two handlers that differ only in that
  spelling are now recognised as the same handler. This applies to every property access in a
  handler, not only assertions: `page['locator']('x')` matches `page.locator('x')`. Only a static
  string literal whose decoded value is a valid identifier is folded — a dynamic key, a template
  literal, or a name with no dot spelling such as `['not.resolves']` keeps its own identity.
  Optional access is preserved, so `a?.['b']` matches `a?.b` and neither matches `a.b`.
  **Reports can gain findings on upgrade**, because pairs that were previously distinguished only
  by access spelling are now duplicates.

### Fixed

- The regular-expression `v` flag is part of a matcher's identity. It enables Unicode set notation
  and changes character-class semantics, so `/^a gauge$/v` and `/^a gauge$/` match different inputs
  and are no longer reported as `normalized-matcher`. Flags that only affect how a match is executed
  — `g`, `y` and `d` — are still ignored for identity, and two matchers carrying `v` still compare
  as the same matcher.
- Replacing the assertion factory through a second `require()` of the same module now revokes
  assertion trust. Two `require()` calls for one module return the same cached object, so
  `other.expect = replacement` replaces the factory that a separate `api.expect(...)` call uses.
  Previously only a local alias (`const other = api`) propagated, so handlers relying on a factory
  replaced through the second binding were still reported as duplicates.
- A `for (const x of xs)` or `for (const x in xs)` head binds `x` for the loop, so a reference to
  `x` in the body is the loop variable rather than an outer constant of the same name. The binding
  was not registered, so the outer constant's value was substituted into the loop body. This was
  wrong in both directions: identical loop handlers with different unrelated outer constants were
  reported as distinct, and different loop bodies sharing an outer constant could be reported as
  duplicates. `class`, `function`, `catch` and classic `for (let i = …)` bindings were already
  handled.

## [0.4.0] - 2026-09-15

### Changed

- Static computed property access resolves the same way dot access does at every position of an
  assertion chain, so `expect(x)['not'].toBe(y)` now carries the semantics of
  `expect(x).not.toBe(y)`. Conflicting expected values behind a computed modifier are no longer
  collapsed into a `parameterization-candidate`, and a dotted and a computed spelling of the same
  assertion can now be reported as near-duplicates where previously neither was. Reports can
  therefore change in both directions on upgrade.
- A property name resolves only when it is written as a static string literal. A concatenation, a
  template literal, a runtime value, or an escape the decoder does not support stays unresolved
  rather than resolving to a name the source never spells, which keeps the handler non-comparable
  instead of claiming an assertion.

### Fixed

- Writing through an alias of a matcher builder revokes assertion trust for every spelling of that
  alias, not only the flat one. Nested destructuring, shorthand bindings, shorthand bindings
  carrying a default, and object rest bindings are now treated like `const negated =
  api.expect.not`. Handlers that rely on a matcher replaced through one of those aliases are no
  longer reported as duplicates.
- Object rest bindings are treated as the shallow copies they are. `copy.not.objectContaining = x`
  reaches the object the source still references and revokes trust, while `copy.not = x` replaces a
  slot on the copy alone and leaves trust intact. Previously neither revoked it.

## [0.3.0] - 2026-09-14

### Added

- `--baseline-from-ref` and the matching Action input compare against a fetched Git revision
  with the current analysis policy, reusing `--fail-on-new` without a committed baseline file.
  Incomplete historical scans are rejected rather than treated as clean baselines.

## [0.2.1] - 2026-09-13

### Changed

- Release binaries abort on panic and ship without a symbol table, which reduces the published
  archive for each platform by about 21 percent, from 2,499,615 to 1,972,110 bytes on
  `aarch64-apple-darwin`, with no measured analysis slowdown. Only the binaries this project
  distributes are affected; a crate depending on CukeDedup keeps its own release profile.
- Handlers whose behavior is a parameterized inline invocation, such as
  `((value) => check(value))(compute())`, are excluded from handler comparison. The arguments
  are not substituted, so the body is never read and two such handlers are not claimed to be
  equivalent. Version 0.2.0 reported a byte-identical pair of that shape as a duplicate; it is
  no longer reported. Zero-parameter invocations are unaffected and still compare normally.

### Fixed

- Preserve callback call evidence in near-duplicate analysis without promoting deferred assertions
  to executed behavior, preventing unrelated callback bodies from appearing identical.
- Clarify that parameterization suggestions preserve expected assertion values and polarity.
- Near-duplicate analysis now distinguishes assertions targeting different properties, expecting
  different inline or immutable local values, or using opposite chains such as `expect(...).not`,
  while retaining assertion-only duplicate candidates and avoiding misleading handler-overlap
  warnings. Assertion provenance now also respects TypeScript runtime namespaces, erased
  declarations, namespace bindings, and wrapped assertion expressions.
- Changed-files mode no longer inherits the caller's Git environment. `GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_INDEX_FILE`, and related variables outrank the repository selected per invocation, so
  running CukeDedup from another repository's Git hook resolved revisions, ignore rules, and the
  changed set against the hook's repository instead of the analyzed root.

## [0.2.0] - 2026-09-10

### Changed

- Reorganized the user documentation around a concise quick-start README and focused reference
  guides for configuration, discovery, rules, reports, CI adoption, and safety limits.
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
- **Breaking (Rust API):** public result, diagnostic, reporter, and extensible enum types are now
  `#[non_exhaustive]`. Use `AnalysisResult::new`, `ExecutionMetrics::new`, existing constructors,
  and wildcard enum match arms so future fields and variants do not require another breaking
  release.

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

- Repository-owned CodeQL analysis excludes the intentionally malformed JavaScript parser fixture
  from extraction while preserving extended security queries for every supported project language.
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

[0.8.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/figueiredoluiz/cuke-dedup/releases/tag/v0.1.0
