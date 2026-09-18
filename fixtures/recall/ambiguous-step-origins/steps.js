// Purpose: every Gherkin construct that can produce a concrete step must be able to prove an
// ambiguity. Each pair below is reachable from exactly one construct, so a construct that stopped
// contributing steps would lose its finding while the others stayed green.
const { Given, When, Then } = require("@cucumber/cucumber");
Given("the alpha hatch opens {word}", () => a1());
Given("the alpha hatch opens wide", () => a2());
Given("the beta valve turns {word}", () => b1());
Given("the beta valve turns left", () => b2());
Given("the gamma dial reads {word}", () => c1());
Given("the gamma dial reads high", () => c2());
Given("the delta pump runs {word}", () => d1());
Given("the delta pump runs fast", () => d2());
Given("the epsilon lamp glows {word}", () => e1());
Given("the epsilon lamp glows dim", () => e2());
Given("the zeta rotor spins {word}", () => f1());
Given("the zeta rotor spins clockwise", () => f2());
Given("the eta brake holds {word}", () => g1());
Given("the eta brake holds firm", () => g2());

// A Background nested inside a Rule block is a distinct construct from a feature-level Background.
Given("the theta clamp grips {word}", () => h1());
Given("the theta clamp grips tight", () => h2());

// Registration keyword must not gate matching: cucumber-js definitions are keyword-agnostic, so a
// `When` step has to reach a `Given` definition and a `Then` step a `When` one.
Given("the iota shaft drives {word}", () => i1());
Given("the iota shaft drives hard", () => i2());
When("the kappa gear meshes {word}", () => j1());
When("the kappa gear meshes cleanly", () => j2());
Then("the lambda seal closes {word}", () => k1());
Then("the lambda seal closes flush", () => k2());

// A `But` continuation is a fourth continuation form alongside And and the asterisk.
Given("the mu spring loads {word}", () => l1());
Given("the mu spring loads slowly", () => l2());

// Markdown Gherkin steps must report the line they occupy in the markdown source.
Given("the nu piston strokes {word}", () => m1());
Given("the nu piston strokes long", () => m2());

// A localized feature keeps matching on step text, which carries no keyword.
Given("the xi bearing rolls {word}", () => n1());
Given("the xi bearing rolls freely", () => n2());

// Control: one construct, exactly one matching definition, so no ambiguity anywhere.
Given("the omicron latch seats fully", () => o1());
