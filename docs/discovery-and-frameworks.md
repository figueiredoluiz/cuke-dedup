# Discovery and frameworks

CukeDedup combines safe built-in discovery with statically readable project configuration. It
never executes framework configuration, package scripts, or test code.

## Supported registrations

| Workflow | Supported modules and forms |
| --- | --- |
| Cucumber.js | `@cucumber/cucumber` and legacy `cucumber`; ESM, CJS, aliases, namespaces, TypeScript `import x = require()`, and static local re-exports. |
| Playwright-BDD | `createBdd()` registrations from `playwright-bdd`, plus class-method `Given`, `When`, `Then`, and `Step` decorators from `playwright-bdd/decorators`. |
| Cypress Cucumber | `@badeball/cypress-cucumber-preprocessor`, plus legacy `cypress-cucumber-preprocessor/steps`. |
| Jest-Cucumber | `jest-cucumber` named/namespace ESM imports, CommonJS, TypeScript import-equals, and static local re-exports; inline synchronous `defineFeature(feature, test => test(title, steps => …))` callbacks. |
| Vitest Cucumber Plugin | Named/namespace ESM imports and static local re-exports of `Given`, `When`, and `Then` from `vitest-cucumber-plugin`. |

`import cucumber = require('@cucumber/cucumber')` binds the whole module, as `require` does, so
`cucumber.Given(…)` registers. A default import does not establish a registration namespace. The Cucumber, Playwright-BDD, and Cypress CommonJS entries set `__esModule`, so interop does not supply a default namespace; the Vitest plugin's default export configures Vite rather than registering steps. A project module's `export default` value is not modeled. A registration-style member call on a default import is reported as unresolved rather than registered.

CommonJS calls grant registration provenance only when `require` is unshadowed and has no uncertain module-level writes. Runtime bindings remove loader trust in their lexical scope; erased declarations do not. This applies to factories and local-barrel re-exports as well as direct imports. Registration aliases and namespaces use their nearest runtime binding, so a trusted nested alias remains local to its scope and an untrusted local binding masks outer trust. Explicitly configured registration names keep their existing global policy. Import-equals follows the same conservative loader policy because compiler emit mode is not inferred; ordinary ESM imports remain independent of a local `require` binding.

Jest-Cucumber's scenario callback receives `given`, `when`, `then`, `and`, `but`, and `defineStep`. Destructured aliases, options-object members, static property keys, immutable local aliases, and `test.only`/`skip`/`concurrent` are recognized through their lexical origin. Ordinary arrow and function setup callbacks are supported. Reassignments and shadowing remove trust; generator/async setup callbacks, dynamic properties, escaped setup helpers, and `autoBindSteps` remain unsupported and produce incompleteness warnings when reached through recognized framework bindings. Named setup callbacks are not followed. Step handler resolution follows the existing named-handler rules.

Proven Jest-Cucumber loading, parsing, configuration, and code-generation exports, together with the scenario callback's `pending`, are non-registration values. Their immutable aliases and supported re-exports do not by themselves trigger incompleteness. Unknown members and mutated bindings remain conservative; passing registration objects to helper calls can still invalidate their provenance. These exemptions never grant assertion-library trust.

Callback provenance resolution is depth-bounded: an exhausted lookup remains uncertain and can mark the corpus incomplete, even if a longer lookup would establish an ordinary non-registration value. Mutation invalidation follows reachable alias initializers without that depth cutoff; cycles terminate through visited-binding tracking.

Both integrations preserve unshadowed ambient registrations and explicitly configured registration names, with the file's inferred framework attribution. Ordinary function-declaration wrappers that forward matcher and handler parameters unchanged resolve through the actual lexical target, including wrapper chains; their forwarding bodies are not counted as separate definitions. Shadowing, writes, async/generator execution, and dynamic or reordered forwarding do not gain registration trust. This does not enable named Jest setup callbacks or inferred arrow/function-expression wrappers. Registration fallback does not confer assertion-library trust.

Jest-Cucumber binds steps positionally inside a scenario, so its definitions participate only in handler-reuse rules; repeated text is not a global matcher collision. String matchers are reported as `literal`, preserving text such as `{int}` without interpreting it as a Cucumber Expression; regular expressions retain their existing matcher kind. Identical Jest matcher text can still produce `duplicate-handler`. Handler comparisons involving either new framework require the same framework because their handler argument conventions differ. Package recognition does not confer assertion-library trust. `@amiceli/vitest-cucumber` is a separate package and is not supported by this integration.

These integrations use the built-in source and feature patterns. Jest/Vitest runner configuration and dynamic `loadFeature` calls are not executed or used to infer discovery paths; configure `features` and `definitions` explicitly when needed.

Any runtime import of a name — named, default, namespace or `import x = require()` — means that name
is not the ambient registration global. A type-only import is erased, so it does not.

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

Classic and Markdown tables decode `\n`, `\|`, and `\\` and preserve other backslash sequences literally, including Windows paths. Examples values retain their decoded text during outline expansion. Inconsistent table widths and malformed tables fail parsing; valid neighboring feature files remain analyzable, and the report marks the corpus incomplete.

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

Package manifests, scenario tags, and Cypress `stepDefinitions` patterns do not partition the comparison corpus into runtime registries. In particular, scanning a parent directory can report `ambiguous-step` for identical vocabulary in independent suites, even when each suite is unambiguous on its own. Feature-local Cypress registrations need a scan scoped to the definitions loaded for that feature, including any shared steps.

If a narrower root would exclude shared modules or a parent TypeScript configuration, keep the common root and select both inputs explicitly:

```sh
cuke-dedup . --definitions 'packages/accounts/steps/**/*.ts' --features 'packages/accounts/**/*.feature'
```

Include all shared definitions loaded by that suite in the definition patterns. Filtering features alone does not restrict which definitions can match them; filtering definitions alone leaves all discovered features in the corpus. Scope selection must follow the runner's actual registry, not merely suppress an unwanted finding. Check completeness diagnostics after narrowing a scan: no ambiguity findings in an incomplete corpus do not prove that the runtime registry is unambiguous.

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

`--baseline-from-ref` tolerates an incomplete baseline revision by default: if the revision excludes
a generated bundle, contains an unparseable file, or hits a work limit, the comparison still
proceeds against the findings it did extract and a warning notes that a finding the baseline could
not extract may surface as new. Pass `--fail-on-incomplete` to reject an incomplete baseline
instead. A baseline that produced a hard error, that discovered definition files yet extracted no
definitions at all, or that has definitions but no discovered feature files, cannot be subtracted
and always fails regardless of the flag.

The `minified` signal is line geometry, which separates generated output from authored code in
both minified styles — collapsed onto one line, and wrapped at a fixed width. Geometry alone
cannot distinguish a one-line bundle from authored code that happens to be one very long line, so
a file is kept whenever its prefix calls a step registration by name. A bundle that registers
steps is therefore analyzed normally.

Three kinds of evidence keep a file, and any one of them is enough:

1. **A registration module in the prefix** — `@cucumber/cucumber`, `playwright-bdd`,
   `@badeball/cypress-cucumber-preprocessor`, `jest-cucumber`, `vitest-cucumber-plugin`, or their legacy paths. A renaming import leaves no
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

A CommonJS `module.exports`/`exports.X` value is followed when it is an object literal or a
`require`, directly or through a single top-level `const` (`const api = { Given };
module.exports = api`). A value that provably cannot carry a registration is inert and exports
nothing quietly. That covers a literal, and a function or class that never names a registration or a
module other than a Node built-in (`module.exports = async function act() {…}`, `module.exports =
class Page {}`). It also covers an array or object made only of such values, and a top-level
function, class or `const` holding one.

Any other value marks the corpus incomplete rather than resolving silently to nothing, so a barrel
that might hide registrations is reported instead of passing as clean. That includes:
- a factory call or `new` instance;
- a namespace member, or a function that forwards to a registration, including through a bracket
  key (`bdd['Given']`) or a member it picks at runtime (`bdd[name](…)`);
- a `let`/`var` binding, or one whose value may have changed: reassigned, given a property that is
  not inert, or reaching another binding or a call (passed, wrapped, aliased, stored or returned);
- an unresolvable spread or a bracket target (`exports['X']`);
- an export evaluated conditionally at module scope (`if (…) module.exports = …`).

A helper that calls a function it receives as a parameter is inert on the export side: treating
every call through a parameter as a possible registration would flag ordinary callback helpers
such as `items.map(fn)`. The risk is caught where the helper is used instead. A call that passes a
registration as an argument is reported as an unresolved step registration and marks the corpus
incomplete. That covers a direct argument (`helper(Given, 'a step', fn)`, `helper(cucumber.Given,
…)`) and one carried through an argument value unchanged: an object or array
(`helper({ register: Given })`), a ternary branch, an `||`/`&&`/`??` operand, the last operand of a
comma expression, a function or object method that returns or yields it through any control flow, a
constructor argument, or `Given.bind(…)`. An expression that consumes the registration and
yields something new — `Given.name`, `typeof Given`, `Given !== undefined`, `new Given()` — does not
pass it. A registration called inside a callback (`helper(() => Given('a step', fn))`) is extracted
as a definition instead. A name bound locally —
a parameter, a `catch` binding, a loop or block declaration — that merely shares the name, such as
a fixture named `Given`, is a local value and does not count.

Static project configs may use JSONC and `extends` strings or arrays, up to 16 files deep. A
relative base must resolve. A package base — `@example/config/tsconfig.json`, or a bare package
name that follows the package's `tsconfig` field — resolves when it is a workspace package inside
the analysis root, and naming a config that workspace package lacks is an error. Like an `exports`
target, such a base stays inside its package: a subpath or `tsconfig` field with a `..` or
`node_modules` segment (separated by `/` or `\`), an absolute field, or a symlink that leads out of
the package or into its `node_modules` is refused.
However a base is named, one without its suffix (`./tsconfig.base`) means the `.json` file. Any
other package base, typically a shared base such as `@tsconfig/recommended` or `expo/tsconfig.base`
installed in `node_modules`, is skipped rather than failing the config, so the project's own
`baseUrl` and `paths` still apply; an alias defined only by a skipped base stays unresolved and is
reported. Workspace entrypoints honor `exports` before `main` and index-file fallbacks.

Resolution remains inside canonical analysis and package roots. It does not inspect packages in
`node_modules`, and JavaScript configuration inheritance is not evaluated: a base with a script
extension is rejected.

## Incomplete corpora

An incomplete corpus cannot prove that a definition is unused or that no duplicate exists.
CukeDedup therefore reports the condition instead of presenting missing evidence as a clean run.

The corpus is marked incomplete when, for example:

- An imported registration module cannot be resolved statically.
- A discovered source file cannot be parsed (tolerated by default; `--fail-on-unparseable` fails it).
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

## Cucumber-Ruby

Ruby analysis is opt-in: select one Cucumber suite's definition and feature roots explicitly. Include support files because they can register steps too:

```sh
cuke-dedup path/to/project --definitions 'features/**/*.rb' --features 'features/**/*.feature' --require-definitions --require-features --fail-on-incomplete
```

Supported registrations are direct top-level `Given`, `When`, `Then`, `And`, and `But` calls with static string or regex matchers and inline `do/end` or brace blocks. Parenthesized and command-call syntax are accepted. Strings are Cucumber Expressions; slash-delimited and `%r{...}` literals are Ruby regexes. Double-quoted Unicode escapes (`\uXXXX` and `\u{...}` scalar lists) are decoded before matcher comparison; single-quoted escapes retain their literal meaning. Invalid Unicode scalars remain unsupported. The adapter never executes Ruby, Gemfiles, hooks or dependency code. Explicit source selection establishes the intended DSL context; it does not resolve every possible runtime override.

Immutable local method captures such as `given = method(:Given)` followed by top-level `given.call('a step') { work() }` are also supported. Every use of the local binding must be a subsequent direct `.call`; reassignment, shadowing, passing the method object elsewhere, and nested calls remain unsupported and incomplete. The same static matcher and inline-block requirements apply, and method lookup or invocation overrides invalidate registration trust.

Resolved source loads also support immutable top-level registration captures and constant re-exports through simple loaded modules, such as `ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end`. Receiverless captures inside module or class bodies are unsupported because their receiver is not top-level main. A subsequent `register = Provider::GIVEN; register.call('a step') { work() }` uses the captured registrar. The load must precede the reference; an existing but unloaded file is insufficient. Namespace or constant replacement, escaped references, conflicting definitions, and uncertain method lookup prevent positive resolution. This does not support arbitrary namespace factories, cross-file local variables, or runtime-generated providers. Library consumers call `SourceAdapter::prepare_session` with the selected files and resolved dependency edges before session extraction to establish this evidence.

Local forwarding helpers are supported when a top-level method takes one matcher parameter and one block parameter, and its entire body forwards them unchanged to a registration: `def given_step(text, &handler); Given(text, &handler); end`. A later top-level call in the same file may supply a static matcher and inline block. Uncalled forwarding declarations do not create definitions. Extra effects, transformed arguments, nested calls and cross-file invocations remain unsupported. Method replacement or unresolved references across selected files invalidate wrapper trust; same-spelled methods in unrelated scopes may conservatively prevent discovery.

The initial rules include duplicate matchers, exact supported handler reuse, and usage/ambiguity/overlap for statically supported matchers. Handler fingerprints preserve syntax-tree structure, literals, receivers, state writes and operation order. Lexically equivalent parameters and locals may be renamed; method/property names, captured bindings and shadowing remain significant. Bare helper calls are distinguished from local reads. This does not infer RSpec assertion equivalence. Nested blocks, lambdas, interpolation and rescue/ensure paths retain their exact tree structure. Captured top-level locals require the same source-file context; block parameters and block-local declarations preserve shadowing. Source-location values remain context-sensitive without embedding absolute paths in fingerprints. Dynamic lexical access, method/class/module definitions, heredocs and unresolved binding forms remain non-comparable and mark analysis incomplete. Near-wording findings require exact-equal supported Ruby handlers and compatible capture context; internal matcher metadata does not contribute to wording similarity. Parameterization supports straight-line handlers made entirely of calls on explicit simple receivers with static string or numeric arguments and no block parameters. It preserves receiver bindings, method names, call order, argument counts, literal types and safe-navigation differences. Nested/deferred calls, assertions requiring nested call syntax, dynamic arguments, control flow and state writes retain exact structural identity; no assertion-library trust is inferred. Other similarity across different handler trees remains unsupported. Cucumber Expression comparison shares the language-independent Unicode, whitespace and placeholder normalization policy; executable matcher text remains unchanged. Ruby regex comparison uses the supported native translation to normalize named capture spelling and escaped spaces while preserving capture arity, anchors, quantifiers and the complete Ruby flag set. Flag order and repetition do not affect identity; encoding flags remain distinct. Unsupported patterns retain their source identity.

Regex matching supports the conservative Ruby-to-Rust subset with Ruby's line anchors, noncapturing groups, ASCII digit/word/space classes and their complements. ASCII punctuation identity escapes and escaped spaces retain their literal meaning inside and outside character classes. Unique ASCII named captures (`(?<name>...)` and `(?'name'...)`) preserve capture order; ordinary groups become noncapturing when any named group is present, as in Ruby. Literal braces remain literal; supported repetition bounds, including `{,n}`, preserve Ruby matching semantics. Ruby's `m` option enables dot-all independently of line anchors. The `i` option (alone or with `m`) supports ASCII literal patterns with optional line anchors, excluding the full-case-fold sequences `ss`, `ff`, `fi`, `fl`, and `st` (including `ffi` and `ffl`). Ruby can match those sequences against single Unicode characters, which Rust regex cannot reproduce through simple folding. Non-ASCII patterns, escapes, classes, groups and quantifiers with `i` remain indeterminate. The `u` encoding flag supports UTF-8 patterns; `n` supports ASCII-source patterns against the analyzer's UTF-8 feature text. Non-ASCII binary patterns, conflicting encoding flags, other encodings, duplicate capture names, lookarounds, backreferences, possessive quantifiers, unsupported escapes and character-class extensions remain indeterminate. Unsupported matcher execution does not justify unused or overlap findings; the same literal matcher can still establish an exact duplicate.

Dynamic or nested registrations and local DSL replacement are not trusted. Unsupported self-qualified registration calls produce incomplete-discovery diagnostics. Lexically isolated module method definitions and ordinary aliases preserve top-level registration trust. Qualified declaration prefixes require preceding local namespace declarations; missing or shadowed prefixes never establish ownership through spelling. Namespace aliases, owner escape, dynamic installation and unresolved reflective receivers remain conservative. Detected mutations that can affect the registrar invalidate Ruby registrations across the selected suite. Literal top-level `require_relative` calls and `require` calls against explicit `rubyLoadPaths` are followed as a bounded source graph. Autoload is an unresolved executable dependency: eager autoload registration withholds suite registration trust, while deferred autoload preserves initial registrations but makes analysis incomplete. Missing or unsupported dependency loads remain incomplete; external providers such as `aruba/cucumber` require their source roots and supported registration semantics. Supported local definitions can still produce findings, but zero findings does not establish a clean full suite. Do not combine independent Ruby suites or Ruby and JS/TS sources in one analysis; mixed source languages are rejected before comparison. Default source discovery remains JS/TS-only.

Literal Ruby `step` calls, including calls in selected support files, prevent matching definitions from being reported unused. Deferred calls count as possible usage; no reachability proof is attempted. Dynamic calls, unresolved receivers, `steps`, local delegation-method replacements, unresolved dependencies, and unreadable selected Ruby sources disable Ruby unused findings and report incomplete analysis. A resolved dependency removes only its own loading uncertainty; dynamic delegation in that source still prevents authoritative unused claims. Indirect calls do not increase feature counts or establish feature ambiguity. Library consumers should merge `Extraction.indirect_usage` across all selected Ruby files and use `analysis::analyze_with_step_usage`; older analysis entry points conservatively report incomplete Ruby usage without this metadata.

Closed local instances of a preceding, loaded class without inheritance or a custom class-level `new` or `allocate` factory may dispatch literal method names without invalidating unrelated Cucumber registrations. The binding must remain unchanged and unescaped, and the target must be declared on that class. This establishes receiver isolation, not registration or assertion trust.

Ruby reflection is classified by statically resolved method targets: unrelated literal method updates preserve registrations, while uncertain eager dispatch and registration/dispatch API replacement invalidate the suite. Direct inline `ParameterType` transformer bodies are deferred, so dynamic property access there does not invalidate adjacent registrations. Unsupported parameter-type forms and dynamic indirect usage remain incomplete; deferred transformer handling does not establish transformer equivalence or assertion trust.

Direct top-level Ruby `ParameterType` declarations support literal custom names, string/regexp/array patterns, and deferred lambda/proc transformers without executing them. Source patterns supersede manual fallback patterns; duplicate names and unresolved declarations remove matching authority and mark the corpus incomplete. Unknown names and built-in redefinitions invalidate registration trust. Regexp flags, nested arrays, immediate transformer invocation and preferential-regexp registration remain unsupported. `Extraction.parameter_types` exposes declaration metadata to library consumers separately from handlers; CLI analysis merges the complete selected registry before expression matching.
