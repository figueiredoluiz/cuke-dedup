# Feature discovery

When `features` is not configured, CukeDedup reads literal feature paths from supported BDD setup:

- Playwright-BDD `defineBddConfig` calls.
- Cypress `e2e.specPattern` when the Badeball Cucumber preprocessor is installed.
- Cucumber.js configuration.

Cucumber.js `paths` and Playwright-BDD `features` entries that exist are read as files or
directories as they are on disk; a missing entry is a directory when it ends in `/` or has no
extension. Trailing `:<line>` suffixes are dropped. Cucumber directories include `.feature` and
`.feature.md`. Playwright-BDD directories follow its `.feature` default. Files and globs are
preserved, so projects can select conventions such as `*.spec`. Cypress `e2e.specPattern` values
are used as written, less a leading `./`. Dynamic paths produce a warning; set `features` in
CukeDedup configuration to make them deterministic.

`.feature.md` selects the Gherkin Markdown parser. Other extensions selected by a custom or
framework pattern are parsed as classic Gherkin. This avoids treating ordinary Markdown as
Gherkin while allowing custom names.

Classic and Markdown tables decode `\n`, `\|`, and `\\` and preserve other backslash sequences literally, including Windows paths. Examples values retain their decoded text during outline expansion. Inconsistent table widths and malformed tables fail parsing; valid neighboring feature files remain analyzable, and the report marks the corpus incomplete.

A Scenario Outline step is expanded once per Examples row. Each `<name>` whose name is an Examples
header is replaced by the row's value, wherever it appears in the step text; the first of several
equal headers supplies the value. A `<` that does not open a header name (`the <a<b> value` keeps
`<a`), an unknown name, an unclosed `<` and a header name that contains `>` stay as written. A
substituted value is not substituted again. Doc strings and data tables are not part of the step
text and are not expanded.

A step's span ends one past the last character of its text on the source line, whatever spaces
or tabs separate the keyword from the text. Expanded steps keep the span of their outline line.
Columns count characters. In `.feature.md` files the list marker is replaced by two spaces before
parsing, so columns refer to that converted line.

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
