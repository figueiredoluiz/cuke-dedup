// Controls: shapes that must not be read as registrations, so each pair proves no ambiguity.

// A lookalike package name is not a supported entrypoint.
const { Given: Lookalike } = require("@cucumber/cucumber-extra");
Lookalike("the omicron latch seats {word}", () => p1());
Lookalike("the omicron latch seats fully", () => p2());

// A locally declared function shadows the ambient global.
function Given(pattern, handler) {
  return { pattern, handler };
}
Given("the pi rod extends {word}", () => q1());
Given("the pi rod extends far", () => q2());
