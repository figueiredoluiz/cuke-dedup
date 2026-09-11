# Configuration

CukeDedup works without configuration. Add a project configuration only when the built-in
discovery or policy does not match the repository.

## Precedence

Values are resolved in this order, from highest to lowest priority:

```text
CLI flags
explicit --config
auto-discovered CukeDedup configuration
detected framework configuration
built-in defaults
```

`--config/-c <file>` bypasses automatic discovery. A missing or malformed explicit file is fatal.
Relative configuration, output, and baseline paths are resolved from the analyzed root.

Without `--config`, CukeDedup stops at the first valid source in this order:

```text
.cuke-dedup.json
.config/cuke-dedup.json
.config/.cuke-dedup.json
cuke-dedup.config.json
package.json#cukeDedup
```

An invalid auto-discovered file produces a warning, then discovery continues to the next source.
JavaScript projects may use the `cukeDedup` key in `package.json`; other projects should prefer
`.cuke-dedup.json`.

## Complete example

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

Collection values—`definitions`, `features`, `exclude`, `reporters`, and `suppressions`—replace
the corresponding value from a lower-precedence layer. Scalar settings and individual rule
severities merge by setting or rule name. Configured excludes replace lower-precedence custom
excludes, but built-in generated-directory exclusions remain a separate policy.

## Discovery patterns

`definitions`, `features`, `exclude`, and suppression `path` values use `globset` syntax against
root-relative paths normalized with `/`.

- `*` and `?` may cross `/`.
- `**` makes a recursive directory boundary explicit to readers.
- Character classes such as `[ab]` are supported.
- Brace alternatives such as `{js,ts}` are supported.
- Backslash escaping is supported.

These patterns differ from `.gitignore` and `.cuke-dedupignore`, which use directory-scoped
gitignore semantics.

`excludeDefaults: false` permits an explicitly selected workspace under `node_modules`, `target`,
or another protected directory. `includeHidden: true` permits traversal into dot-directories.
Their CLI equivalents are `--no-default-excludes` and `--include-hidden`. Explicit definition
patterns do not implicitly weaken either safety default.

## Ignore files

A repository may put `.cuke-dedupignore` at the analysis root or in any descendant directory.
Each file applies to its own subtree and follows gitignore syntax:

- Blank lines and `#` comments are ignored.
- A leading `/` anchors the rule to the ignore file's directory.
- A trailing `/` selects directories.
- `!` negates an earlier matching rule.

Example:

```gitignore
# Generated feature sources
features/generated/*
**/*.generated.ts

# Keep one reviewed generated definition
!features/generated/reviewed.generated.ts
```

`.cuke-dedupignore` applies in addition to `.gitignore`, built-in exclusions, and configured
`exclude` patterns. `--no-default-excludes` disables only the built-in list. A configured exclude
is a hard exclusion and cannot be negated from an ignore file.

## Custom registration wrappers

`registrations` names local functions that register steps. CukeDedup already infers a top-level,
synchronous, single-statement wrapper that forwards its leading parameters directly to a known
registration:

```ts
function step(text, handler) {
  Given(text, handler);
}
```

Nested, asynchronous, generator, conditional, multi-statement, reordered, rewritten, or
dynamically constructed wrappers cannot be inferred safely. Declare those names when they still
take the matcher first and handler second.

## Custom parameter types

`parameterTypes` maps a project-defined Cucumber Expression parameter type to the regular
expression it accepts. This mirrors `defineParameterType` without executing project code:

```json
{
  "parameterTypes": {
    "colour": "red|green|amber"
  }
}
```

An undeclared type uses a permissive fallback that prevents a false `unused-definition` finding
but cannot prove ambiguity or overlap. Declaring it restores exact analysis. Invalid expressions
fail configuration rather than silently disabling checks.

## Suppressions

Every configured suppression must select at least a `path` or `matcher` and include a non-empty
reason of at most 512 Unicode characters.

- When both selectors are present, both must match the same definition.
- For pair findings, a path must contain every involved definition.
- A matcher must select at least one involved definition.

This prevents a directory exception from hiding a conflict that crosses into maintained code.

A source-local suppression can instead be placed immediately above a registration:

```ts
// cuke-dedup:ignore duplicate-handler -- retained for an external compatibility contract
Given("the legacy flow completes", legacyHandler);
```

Malformed directives and oversized reasons are operational errors. A directive applies only to
findings involving its attached definition.

## CLI overrides and diagnostics

Discovery and rule options can be overridden directly:

```sh
cuke-dedup . \
  --config .cuke-dedup.json \
  --definitions 'features/steps/**/*.ts' \
  --features 'features/**/*.feature' \
  --exclude 'generated/legacy.ts' \
  --exclude 'vendor' \
  --threshold 5 \
  --require-definitions \
  --rule duplicate-matcher=warning
```

`--exclude` may be repeated or receive comma-separated patterns.

Use `--explain-discovery` to show effective pattern origins, selected parsers, matching patterns,
and definition inputs without changing report output. Use `--print-config` to print the fully
merged and validated configuration as JSON, including its sources and warnings, then exit without
running discovery.

See [Discovery and frameworks](discovery-and-frameworks.md) for framework-derived defaults and
incomplete-corpus behavior, and [Rules](rules.md) for severity configuration.
