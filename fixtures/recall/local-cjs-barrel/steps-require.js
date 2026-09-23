// The same CommonJS barrel imported through a local `require('./barrel')` rather than an ESM
// specifier. The importer path must resolve a project-local `require` too, or these definitions
// vanish exactly as the ESM case did before the barrel was modeled. `require('@cucumber/cucumber')`
// is a package; `require('./barrel')` is a project-local module that only resolves through the
// resolver. Distinct handler bodies keep this pair from colliding with the ESM steps by handler.
const { Given: registerViaRequire } = require("./barrel");

registerViaRequire("cjs barrel require registration", async () => thirdOperation());
registerViaRequire("cjs barrel require registration", async () => fourthOperation());
