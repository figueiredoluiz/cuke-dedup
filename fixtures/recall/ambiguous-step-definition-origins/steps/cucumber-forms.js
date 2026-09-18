// Purpose: every import shape that reaches a supported registration export must be able to prove an
// ambiguity. Each pair is registered through exactly one shape, so a shape the resolver stopped
// recognizing loses its finding while the others stay green.
const { Given } = require("@cucumber/cucumber");
const legacy = require("cucumber");
const { defineStep } = require("@cucumber/cucumber");
const { Given: Aliased } = require("@cucumber/cucumber");

// CommonJS destructured require.
Given("the alpha hatch opens {word}", () => a1());
Given("the alpha hatch opens wide", () => a2());

// Namespace require plus member call, on the legacy module entrypoint.
legacy.Given("the beta valve turns {word}", () => b1());
legacy.Given("the beta valve turns left", () => b2());

// The `defineStep` export.
defineStep("the gamma dial reads {word}", () => c1());
defineStep("the gamma dial reads high", () => c2());

// A locally renamed import.
Aliased("the delta pump runs {word}", () => d1());
Aliased("the delta pump runs fast", () => d2());
