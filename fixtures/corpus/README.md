# CukeDedup test corpus

This directory contains small, sanitized repositories used as end-to-end fixtures. Each case is independent and may be analyzed directly with the CukeDedup CLI.

`manifest.json` records the command arguments and expected machine-report summary for every case. The integration suite copies each case into a temporary repository and executes the manifest rather than maintaining a second hard-coded list of fixtures. Optional `materialize` entries create inputs that intentionally cannot be tracked at their runtime path, such as files excluded by a fixture's `.gitignore`.

The corpus includes classic and localized Gherkin, Unicode source paths, Gherkin Markdown, every registered JavaScript/TypeScript source suffix, mixed ESM/CJS and framework registrations, Playwright-BDD and Cypress Cucumber project configuration, local Playwright-BDD fixtures reached through TypeScript path aliases, injected-fixture handler shapes, nonstandard framework-selected extensions, threshold boundaries, `.gitignore` and `.cuke-dedupignore` behavior, explicit discovery overrides, cross-package discovery, malformed matcher diagnostics, and adversarial parser and named-handler combinations. Every case carries labels (`role`, `origin`, `language`, `frameworks`, `verdict`, `classifiedAt`, `axes`) described in the [recall corpus README](../recall/README.md); the corpus check holds `language` and `frameworks` against the run's `--emit-fingerprints` records and `role` against the required findings. It must not contain proprietary source code, credentials, generated reports, or copied third-party repositories. Add an original adversarial case whenever a parser, discovery, policy, or reporting bug is fixed.

## Replay a structured derivative

The seed renderer starts from the reviewed `mixed-esm-and-cjs-registration-syntax` case. It models one scenario step and two logical definition owners, `prepare` and `restore`. The input bundle records their rendered locations, configuration arguments, expected finding participants, ordered transformations, applicability, verdict and a SHA-256 digest. The base reproduces the checked-in fixture. Every derivative is labelled `generated`.

```sh
node scripts/evaluate/corpus-seed.mjs render move-module extract-helper 'value=café bleu' outline > case.json
node scripts/evaluate/corpus-seed.mjs replay case.json
```

Replay verifies the version and digest, writes the saved UTF-8 files to a new temporary directory and prints its path. The caller owns that directory. Replay does not load the generator or execute the project. To exercise the authored cases against the analyzer, build with `cargo build --release` and run `node --test scripts/evaluate/corpus-seed.integration.mjs`.

The bounded operations are `move-module`, `extract-helper`, `value=<words>` (letters, digits and single spaces), `outline`, `disable-ambiguity` and `restore-ambiguity`. The last requires the file-level disable and restores the rule at warning severity through the CLI. Two terminal operations are available. `malformed-config` explicitly selects a broken configuration and expects a parse failure. `unknown-framework` produces an unsupported input with no semantic expectation.
