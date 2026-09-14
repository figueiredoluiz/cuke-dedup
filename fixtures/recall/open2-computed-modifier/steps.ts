import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";

// OPEN-2 / R5190030268-S2.
//
// A statically computed modifier names the same modifier as dot access. Conflicting expected
// values behind `['not']` must stay distinguishable, exactly as they do behind `.not`.
Then("the computed panel shows the first state", async ({ page }) => {
  expect(page.status)["not"].toBe("ready");
});

Then("the computed panel shows the final state", async ({ page }) => {
  expect(page.status)["not"].toBe("idle");
});

// Positive control on a different subject, so it cannot be drawn into the pair above. Equal
// expected values behind the same computed modifier are a genuine duplicate, so this case cannot
// pass merely by the analyzer ignoring computed modifiers altogether. The wording is deliberately
// far apart so only `duplicate-handler` applies.
Then("the inventory badge is settled", async ({ page }) => {
  expect(page.badge)["not"].toBe("ready");
});

Then("the archive marker has finished loading", async ({ page }) => {
  expect(page.badge)["not"].toBe("ready");
});

// Execution-effect control. Negation changes what the assertion means, so a computed negation must
// never collapse into the unmodified assertion. The wording is deliberately close and non-opposing:
// only the differing assertions keep these apart, so an implementation that unwrapped `['not']`
// while discarding the modifier would report them as a near duplicate.
Then("the gauge reading is checked", async ({ page }) => {
  expect(page.gauge)["not"].toBe("ready");
});

Then("the gauge reading is now checked", async ({ page }) => {
  expect(page.gauge).toBe("ready");
});
