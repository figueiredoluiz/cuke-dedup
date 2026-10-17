# Generated and non-source content

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

Because classification reads a bounded prefix, a bundle larger than the 8 MiB input limit whose
prefix matches an exclusion signal is excluded with this warning rather than ending the run. A
large file that the evidence below keeps is parsed, and then fails the input limit as an
operational error.

`--baseline-from-ref` tolerates an incomplete baseline revision by default: if the revision excludes
a generated bundle, contains an unparseable file, or hits a work limit, the comparison still
proceeds against the findings it did extract and a warning notes that a finding the baseline could
not extract may surface as new. Pass `--fail-on-incomplete` to reject an incomplete baseline
instead. A baseline that produced a hard error, that has definitions but no discovered feature
files, or that kept no definitions while the current tree has some (so no finding could be matched),
cannot be subtracted and always fails regardless of the flag. When neither tree has definitions,
for example a Ruby registry withdrawn by an unresolved `include` in both, there is nothing to
compare and the run follows the plain-run incompleteness policy.

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

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
