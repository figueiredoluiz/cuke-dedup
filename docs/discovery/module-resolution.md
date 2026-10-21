# Static module resolution

Registration imports can be resolved through:

- Relative JavaScript and TypeScript modules.
- The nearest `tsconfig.json` or `jsconfig.json` `paths` mappings.
- The nearest package's `imports` mappings.
- Packages declared by the analysis root's `workspaces`.

Resolution follows only ECMAScript source suffixes (`.ts`, `.tsx`, `.mts`, `.cts`, `.js`, `.jsx`,
`.mjs`, `.cjs`). An import of any other file (`.rb`, `.json`, `.css`, `.feature`), of a declaration
file (`.d.ts`) or of a directory with no ECMAScript index resolves to nothing: the file is not read
or parsed, a barrel that re-exports it keeps its other re-exports, and a registration name imported
from it is reported as an unresolved import, as for any other unresolved module.

A resolved barrel re-exports registrations through either module system:

- ESM `export { X } from './a'`, `export * from './a'`, aliased re-exports, and a split
  `import { X } from './a'; export { X }`.
- CommonJS `module.exports = { X }`, `module.exports = require('./a')`,
  `module.exports = { ...require('./a'), X }`, `Object.assign(module.exports, require('./a'))`,
  `module.exports.X = X`, and `exports.X = require('./a').X` (or a local binding). Only assignments
  in the module body count; the same syntax inside a function or class is not a module export.

A CommonJS `module.exports`/`exports.X` value is followed when it is an object literal or a
`require`, directly or through a single top-level `const` (`const api = { Given };
module.exports = api`). A value that provably cannot carry a registration is inert and exports
nothing quietly. That covers a literal, and a function or class that never names a registration or a
module other than a Node built-in (`module.exports = async function act() {…}`, `module.exports =
class Page {}`). It also covers an array or object made only of such values, and a top-level
function, class or `const` holding one.

Any other value marks the corpus incomplete rather than resolving silently to nothing, so a barrel
that might hide registrations is reported instead of passing as clean. That includes:
- a factory call or `new` instance;
- a namespace member, or a function that forwards to a registration, including through a bracket
  key (`bdd['Given']`) or a member it picks at runtime (`bdd[name](…)`);
- a `let`/`var` binding, or one whose value may have changed: reassigned, given a property that is
  not inert, or reaching another binding or a call (passed, wrapped, aliased, stored or returned);
- an unresolvable spread or a bracket target (`exports['X']`);
- an export evaluated conditionally at module scope (`if (…) module.exports = …`).

A helper that calls a function it receives as a parameter is inert on the export side: treating
every call through a parameter as a possible registration would flag ordinary callback helpers
such as `items.map(fn)`. The risk is caught where the helper is used instead. A call that passes a
registration as an argument is reported as an unresolved step registration and marks the corpus
incomplete. That covers a direct argument (`helper(Given, 'a step', fn)`, `helper(cucumber.Given,
…)`) and one carried through an argument value unchanged: an object or array
(`helper({ register: Given })`), a ternary branch, an `||`/`&&`/`??` operand, the last operand of a
comma expression, a function or object method that returns or yields it through any control flow, a
constructor argument, or `Given.bind(…)`. An expression that consumes the registration and
yields something new — `Given.name`, `typeof Given`, `Given !== undefined`, `new Given()` — does not
pass it. A registration called inside a callback (`helper(() => Given('a step', fn))`) is extracted
as a definition instead. A name bound locally —
a parameter, a `catch` binding, a loop or block declaration — that merely shares the name, such as
a fixture named `Given`, is a local value and does not count.

Static project configs may use JSONC and `extends` strings or arrays, up to 16 files deep. A
relative base must resolve. A package base — `@example/config/tsconfig.json`, or a bare package
name that follows the package's `tsconfig` field — resolves when it is a workspace package inside
the analysis root, and naming a config that workspace package lacks is an error. Like an `exports`
target, such a base stays inside its package: a subpath or `tsconfig` field with a `..` or
`node_modules` segment (separated by `/` or `\`), an absolute field, or a symlink that leads out of
the package or into its `node_modules` is refused.
However a base is named, one without its suffix (`./tsconfig.base`) means the `.json` file. Any
other package base, typically a shared base such as `@tsconfig/recommended` or `expo/tsconfig.base`
installed in `node_modules`, is skipped rather than failing the config, so the project's own
`baseUrl` and `paths` still apply; an alias defined only by a skipped base stays unresolved and is
reported. Workspace entrypoints honor `exports` before `main` and index-file fallbacks.

Resolution remains inside canonical analysis and package roots. It does not inspect packages in
`node_modules`, and JavaScript configuration inheritance is not evaluated: a base with a script
extension is rejected.

Back to [Discovery and frameworks](../discovery-and-frameworks.md).
