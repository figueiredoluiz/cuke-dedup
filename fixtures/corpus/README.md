# CukeDedup test corpus

This directory contains small, sanitized repositories used as end-to-end fixtures. Each case is independent and may be analyzed directly with the CukeDedup CLI.

`manifest.json` records the command arguments and expected machine-report summary for every case. The integration suite copies each case into a temporary repository and executes the manifest rather than maintaining a second hard-coded list of fixtures. Optional `materialize` entries create inputs that intentionally cannot be tracked at their runtime path, such as files excluded by a fixture's `.gitignore`.

The corpus includes classic and localized Gherkin, Unicode source paths, Gherkin Markdown, every registered JavaScript/TypeScript source suffix, Playwright-BDD and Cypress Cucumber project configuration, nonstandard framework-selected extensions, threshold boundaries, `.gitignore` and `.cuke-dedupignore` behavior, explicit discovery overrides, cross-package discovery, malformed matcher diagnostics, and adversarial parser and named-handler combinations. It must not contain proprietary source code, credentials, generated reports, or copied third-party repositories. Add an adversarial case whenever a parser, discovery, policy, or reporting bug is fixed.
