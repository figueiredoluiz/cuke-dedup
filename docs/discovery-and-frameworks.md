# Discovery and frameworks

CukeDedup combines safe built-in discovery with statically readable project configuration. It
never executes framework configuration, package scripts, or test code.

Each section below summarizes one page under `docs/discovery/`.

## Supported registrations

Registration forms recognized per framework, how imports and aliases establish provenance, and what remains unsupported. Details: [discovery/registrations.md](discovery/registrations.md).

## Feature discovery

How feature paths are read from Playwright-BDD, Cypress and Cucumber.js setup when `features` is not configured, and which parser each extension selects. Details: [discovery/features.md](discovery/features.md).

## Corpus boundaries

An analysis root usually defines one comparison corpus. [Automatically discovered mixed Ruby and Cucumber.js suites](#cucumber-ruby) use separate comparison corpora and combine their findings in one report. For monorepos and feature-local registries, scope both features and definitions to the suite. Details: [discovery/corpus-boundaries.md](discovery/corpus-boundaries.md).

## Generated and non-source content

Compressed, binary and minified sources are excluded before parsing, marked incomplete and reported. The three kinds of evidence that keep a file. Details: [discovery/generated-content.md](discovery/generated-content.md).

## Static module resolution

Relative modules, `tsconfig`/`jsconfig` paths, package `imports` and workspaces; ESM and CommonJS barrels; which export shapes resolve, which are inert, and which mark the corpus incomplete. Details: [discovery/module-resolution.md](discovery/module-resolution.md).

## Incomplete corpora

What marks a corpus incomplete, how reports expose it, and the flags that turn incompleteness into a failure. Details: [discovery/incomplete-corpora.md](discovery/incomplete-corpora.md).

## Cucumber-Ruby

Experimental Ruby support covers automatic discovery for conventional suites, including mixed repositories with separate feature selections, explicit selection, registrations, and static dependency analysis. [Cucumber-Ruby support](discovery/cucumber-ruby.md)

Sections: [Registrations and providers](discovery/cucumber-ruby.md#registrations-and-providers), [Handlers and assertion evidence](discovery/cucumber-ruby.md#handlers-and-assertion-evidence), [Matchers](discovery/cucumber-ruby.md#matchers), [Ownership, dependencies and usage](discovery/cucumber-ruby.md#ownership-dependencies-and-usage), [Parameter types](discovery/cucumber-ruby.md#parameter-types).
