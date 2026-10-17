# Incomplete corpora

An incomplete corpus cannot prove that a definition is unused or that no duplicate exists.
CukeDedup therefore reports the condition instead of presenting missing evidence as a clean run.

The corpus is marked incomplete when, for example:

- An imported registration module cannot be resolved statically.
- A recognized registration has a dynamic, missing or malformed matcher, so its definition cannot be extracted. Other valid definitions and findings remain available.
- A discovered source file cannot be parsed (tolerated by default; `--fail-on-unparseable` fails it).
- A converted Gherkin Markdown file parses but yields no concrete steps.
- Candidate analysis reaches a configured or hard work limit.

Calls to proven empty local object methods are excluded from registration uncertainty, including immutable aliases. Unknown or effectful methods, mutations, escaped objects, dynamic properties and prototype/coercion hooks remain conservative; a matching method name alone never establishes Cucumber provenance.

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

Malformed included JavaScript or TypeScript warns and marks the corpus incomplete by default;
error-recovered definitions are kept. Partial extraction could make a duplication gate pass
incorrectly, so use `--fail-on-unparseable` to fail on parse errors or `--fail-on-incomplete` to
reject any incomplete run. Fix the syntax, use `.tsx` for JSX-bearing TypeScript, or narrow
`definitions`. A statically unresolved module is a non-fatal limitation by default; a module that
the filesystem refuses to read is an operational error.

See [Configuration](../configuration.md) for explicit patterns and [Safety and limitations](../safety-and-limitations.md)
for resolution and input bounds.

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
