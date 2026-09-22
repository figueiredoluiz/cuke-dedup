# Discovery and frameworks

CukeDedup combines safe built-in discovery with statically readable project configuration. It
never executes framework configuration, package scripts, or test code.

## Supported registrations

| Workflow | Supported modules and forms |
| --- | --- |
| Cucumber.js | `@cucumber/cucumber` and legacy `cucumber`; ESM, CJS, aliases, namespaces, and static local re-exports. |
| Playwright-BDD | `createBdd()` registrations from `playwright-bdd`, plus class-method `Given`, `When`, `Then`, and `Step` decorators from `playwright-bdd/decorators`. |
| Cypress Cucumber | `@badeball/cypress-cucumber-preprocessor`, plus legacy `cypress-cucumber-preprocessor/steps`. |

Package entrypoints are matched exactly. The root of the legacy Cypress package is not a step
registration module; use its `/steps` entrypoint. Plain Playwright projects are supported when
their Gherkin bindings use Cucumber.js or Playwright-BDD.

## Feature discovery

When `features` is not configured, CukeDedup reads literal feature paths from supported BDD setup:

- Playwright-BDD `defineBddConfig` calls.
- Cypress `e2e.specPattern` when the Badeball Cucumber preprocessor is installed.
- Cucumber.js configuration.

Cucumber directories include `.feature` and `.feature.md`. Playwright-BDD directories follow its
`.feature` default. Explicit file and glob paths are preserved, so projects can select conventions
such as `*.spec`. Dynamic paths produce a warning; set `features` in CukeDedup configuration to
make them deterministic.

`.feature.md` selects the Gherkin Markdown parser. Other extensions selected by a custom or
framework pattern are parsed as classic Gherkin. This avoids treating ordinary Markdown as
Gherkin while allowing custom names.

## Corpus boundaries

One analysis root is one comparison corpus. Every discovered definition and feature beneath that
root can participate in the same result.

For a monorepo whose packages have independent step registries, run CukeDedup separately:

```sh
cuke-dedup packages/accounts
cuke-dedup packages/billing
```

CukeDedup does not split a root automatically by detected framework because a mixed-framework
project can deliberately share definitions.

## Generated and non-source content

Discovery selects definition sources by path, so vendored bundles, compressed payloads and binary
blobs are picked up whenever they carry a source extension. None of them hold authored step
definitions, so CukeDedup inspects the first 64 KiB of each discovered source and excludes three
kinds before parsing:

| Excluded as | Signal |
| --- | --- |
| `compressed` | The file opens with a gzip, zip, bzip2, xz, zstd, or lz4 stream signature. |
| `binary` | The inspected prefix contains a NUL byte. |
| `minified` | Non-blank lines average more than 200 bytes, and the prefix shows no sign of registering steps. |

Every one of these signals reads file content, and none of them is a proof. JavaScript permits a
NUL byte or archive-like bytes inside a comment or string, and minified geometry cannot be told
apart from one very long authored line. So an exclusion always leaves the analyzed corpus short of
everything discovered, and the run is marked **incomplete**: `--fail-on-incomplete` then fails with
exit code 2, so a strict gate never passes while a discovered file went unanalyzed.

Every exclusion is reported as a warning naming the file and the reason. An exclusion is
never an error by itself; only `--fail-on-incomplete` turns one into a failure:

```
cuke-dedup: warning: excluded 2 discovered definition source file(s) as generated or non-source
content: public/vendor.js (minified), public/blob.js (binary)
```

Because classification reads a bounded prefix, a bundle larger than the 8 MiB input limit is
excluded rather than ending the run.

`--baseline-from-ref` requires a complete baseline corpus, so it fails when the baseline revision
contains a file that would be excluded — the comparison would otherwise subtract findings from a
corpus that was never fully analyzed. Add the generated paths to `exclude`, or narrow
`definitions`, before comparing against a reference.

The `minified` signal is line geometry, which separates generated output from authored code in
both minified styles — collapsed onto one line, and wrapped at a fixed width. Geometry alone
cannot distinguish a one-line bundle from authored code that happens to be one very long line, so
a file is kept whenever its prefix calls a step registration by name. A bundle that registers
steps is therefore analyzed normally.

Three kinds of evidence keep a file, and any one of them is enough:

1. **A registration module in the prefix** — `@cucumber/cucumber`, `playwright-bdd`,
   `@badeball/cypress-cucumber-preprocessor`, or their legacy paths. A renaming import leaves no
   other trace, since `import { Given as G }` calls `G(...)` and no registration name reaches a
   call site.
2. **An import from inside the project** — a specifier starting with `./`, `../`, `~/`, `@/`, or
   `#`, in import position (`from`, `import`, `require(`, `import(`). The project resolver follows
   a registration re-exported by a local module or `tsconfig` path, so the callee there can have
   any name and any supported syntax. A bundle has already resolved its own imports and carries no
   such specifier. Both halves of that rule are load-bearing: one reference bundle contains 80
   occurrences of `"./` inside string data, and a bare package name is not evidence because
   bundles do import packages — jQuery even contains `from '` inside an error message.

   A specifier that looks like a package name is therefore not covered, whether it is a workspace
   package such as `@myorg/steps` or a `tsconfig` mapping such as `@steps/*`. If a project reaches
   its registrations only that way *and* has sources wide enough to look minified, add the local
   callee name to `registrations` so the third kind of evidence applies.
3. **A call to a registration name.** The name must begin an identifier and be followed by a call,
   so `promisedPatchThen(` does not read as `Then(`:

   | Names | Qualified call |
   | --- | --- |
   | `Given`, `When`, `Then`, `And`, `But`, `Step`, `defineStep`, `createBdd`, and any name in `registrations` | Keeps the file, because a namespace import makes `cucumber.Given(...)` a registration. |
   | `given`, `when`, `then` | Does not keep the file: `.then(...)` is a promise continuation, not a registration. A bare `then(...)` does keep it. |

   Everything the analyzer treats as transparent around a callee is skipped between the name and
   its arguments — whitespace, comments, parentheses, non-null `!`, instantiation `<T>`, `as` and
   `satisfies` — so `Given /* matcher */ ('a step', h)`, `(Given)('a step', h)`, `Given!(...)`,
   `Given<string>(...)` and `(Given as typeof Given)(...)` all count.

This evidence is lexical rather than a parse, so it is best effort in both directions. It does not
distinguish a name in executable code from the same bytes inside a string or comment, and it does
not parse, so it recognizes the transparent callee wrappers by their bytes rather than by syntax.
The asymmetry is
deliberate: evidence found where it does not execute only keeps a file that would otherwise be
skipped, costing analysis time, while evidence the scan misses excludes a file and loses its
definitions. That is why an exclusion always marks the corpus incomplete.

Invalid UTF-8 is deliberately not a signal on its own: a mis-encoded but authored file can hold
definitions, so it remains a read error rather than a silent exclusion.

## Static module resolution

Registration imports can be resolved through:

- Relative JavaScript and TypeScript modules.
- The nearest `tsconfig.json` or `jsconfig.json` `paths` mappings.
- The nearest package's `imports` mappings.
- Packages declared by the analysis root's `workspaces`.

A resolved barrel re-exports registrations through either module system:

- ESM `export { X } from './a'`, `export * from './a'`, aliased re-exports, and a split
  `import { X } from './a'; export { X }`.
- CommonJS `module.exports = { X }`, `module.exports = require('./a')`,
  `module.exports = { ...require('./a'), X }`, `Object.assign(module.exports, require('./a'))`,
  `module.exports.X = X`, and `exports.X = require('./a').X` (or a local binding). Only assignments
  in the module body count; the same syntax inside a function or class is not a module export.

A CommonJS `module.exports`/`exports.X` assignment written in a form the analyzer does not model —
a factory call, a namespace member, an aliased object, an unresolvable spread — marks the corpus
incomplete rather than resolving silently to nothing, so a barrel that might hide registrations is
reported instead of passing as clean.

Static project configs may use JSONC and relative JSON `extends` strings or arrays, up to 16 files
deep. Workspace entrypoints honor `exports` before `main` and index-file fallbacks.

Resolution remains inside canonical analysis and package roots. It does not inspect arbitrary
packages in `node_modules`. JavaScript configuration inheritance and package-based `extends` are
not evaluated; use a contained static JSON base config for aliases that affect registration
imports.

## Incomplete corpora

An incomplete corpus cannot prove that a definition is unused or that no duplicate exists.
CukeDedup therefore reports the condition instead of presenting missing evidence as a clean run.

The corpus is marked incomplete when, for example:

- An imported registration module cannot be resolved statically.
- A converted Gherkin Markdown file parses but yields no concrete steps.
- Candidate analysis reaches a configured or hard work limit.

Machine reports expose `corpus.incomplete` and `analysis.truncated`; SARIF marks the invocation
unsuccessful. Baseline updates are refused. By default, valid findings are retained and exit status
still follows their severity. `failOnIncomplete: true` or `--fail-on-incomplete` changes an
incomplete run to operational exit code `2`.

If definitions exist but no feature files match, CukeDedup warns and disables
`unused-definition`. Use `requireFeatures: true` or `--require-features` to fail instead. A valid
classic `.feature` file with no steps is a complete empty input; a Markdown conversion that yields
no steps is incomplete because the converter may have omitted unrecognized content.

If discovery finds definition sources but extracts no definitions, CukeDedup reports that census.
Use `requireDefinitions: true` or `--require-definitions` to make it operationally fatal.

Malformed included JavaScript or TypeScript fails closed because partial extraction could make a
duplication gate pass incorrectly. Fix the syntax, use `.tsx` for JSX-bearing TypeScript, or narrow
`definitions`. A statically unresolved module is a non-fatal limitation by default; a module that
the filesystem refuses to read is an operational error.

See [Configuration](configuration.md) for explicit patterns and [Safety and limitations](safety-and-limitations.md)
for resolution and input bounds.
