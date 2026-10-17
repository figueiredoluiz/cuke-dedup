# Feature discovery

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

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
