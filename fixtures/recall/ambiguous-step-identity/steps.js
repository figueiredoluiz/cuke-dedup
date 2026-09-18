// Purpose: pin the identity of an ambiguity finding — one finding per authored location and
// distinct authoritative match set. Each group below varies one part of that key.
const { Given } = require("@cucumber/cucumber");

// An expression and an authoritative regex accepting one step: two related definitions.
Given("the alpha gauge reads {word}", () => a1());
Given(/^the alpha gauge reads high$/, () => a2());

// Three authoritative matchers accepting one step: three related definitions.
Given("the beta dial holds {word}", () => b1());
Given(/^the beta dial holds .+$/, () => b2());
Given("the beta dial holds firm", () => b3());

// Outline rows sharing one match set coalesce into a single finding.
Given("the gamma pump moves {int}", () => c1());
Given(/^the gamma pump moves \d+$/, () => c2());

// Outline rows with different match sets stay distinct at one template location.
Given("the delta valve turns {word}", () => d1());
Given(/^the delta valve turns left$/, () => d2());
Given(/^the delta valve turns right$/, () => d3());

// The same authored text in two scenarios is two authored locations.
Given("the epsilon lamp glows {word}", () => e1());
Given(/^the epsilon lamp glows dim$/, () => e2());

// An undeclared custom parameter type cannot prove ambiguity.
Given("the zeta rotor spins {speed}", () => f1());
Given("the zeta rotor spins fast", () => f2());

// A single matching definition is not an ambiguity.
Given("the eta brake holds tight", () => g1());
