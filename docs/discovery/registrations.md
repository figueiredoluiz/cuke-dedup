# Supported registrations

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

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
