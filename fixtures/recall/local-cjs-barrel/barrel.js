// A CommonJS barrel that re-exports a registration through `module.exports = { ... }`.
// Before the resolver modeled CJS export assignments this produced zero exports silently, so an
// importer that renamed the registration lost every definition with no diagnostic.
const { Given } = require("@cucumber/cucumber");

module.exports = { Given };
