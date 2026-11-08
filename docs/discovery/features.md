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

Classic and Gherkin Markdown inputs accept one initial UTF-8 byte-order mark before parsing or Markdown conversion. U+FEFF elsewhere remains content. The marker still counts toward the source-file byte limit.

Classic and Markdown tables decode `\n`, `\|`, and `\\` and preserve other backslash sequences literally, including Windows paths. Examples values retain their decoded text during outline expansion. Inconsistent table widths and malformed tables fail parsing; valid neighboring feature files remain analyzable, and the report marks the corpus incomplete.

A CLI run admits at most 250,000 concrete feature steps and 64 MiB of retained step text, keywords and location paths across all selected feature files, including a historical reference baseline when selected. Ordinary scenarios, outlines and feature/rule backgrounds share these limits. Each public extraction call applies the same limits to that call. A feature that exceeds the remaining budget is refused before concrete steps are allocated; it contributes no steps, the corpus is marked incomplete, unused-definition conclusions are disabled and baseline updates are refused. Fitting neighboring files remain analyzable. These limits bound concrete-step retention; source-file parsing and matcher analysis have separate limits.

Outline placeholder scanning has a separate 64 MiB admission allowance per CLI run or public extraction call. Each expanded step charges its original template length before preflight scanning, including when values are empty; materialization can traverse the admitted templates once more. Work already consumed remains charged if a feature is refused, while its retained-step counters roll back. The scan allowance is shared across current and historical feature files. It does not cover source parsing or matcher analysis.

Within those limits, a Scenario Outline step is expanded once per Examples row. Each `<name>` whose name is an Examples header is replaced by the row's value, wherever it appears in the step text; the first of several equal headers supplies the value. A `<` that does not open a header name (`the <a<b> value` keeps `<a`), an unknown name, an unclosed `<` and a header name that contains `>` stay as written. A substituted value is not substituted again. Doc strings and data tables are not part of the step text and are not expanded.

A step's span ends one past the last character of its text on the source line, whatever spaces
or tabs separate the keyword from the text. Expanded steps keep the span of their outline line.
Columns count characters. In `.feature.md` files the list marker is replaced by two spaces before
parsing, so columns refer to that converted line.

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
