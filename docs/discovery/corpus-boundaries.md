# Corpus boundaries

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

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
