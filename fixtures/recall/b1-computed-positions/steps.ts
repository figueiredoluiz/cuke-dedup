import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";

// B1. Static computed access names the same thing as dot access at every position of an assertion
// chain, so each pair below asserts conflicting values and must stay apart. Each pair uses its own
// subject so the pairs cannot be drawn into one another.

// Terminal matcher.
Then("the terminal panel reads the first value", async ({ page }) => {
  expect(page.terminal).not["toBe"]("ready");
});

Then("the terminal panel reads the final value", async ({ page }) => {
  expect(page.terminal).not["toBe"]("idle");
});

// Factory option method.
Then("the option panel reads the first value", async ({ page }) => {
  expect["soft"](page.option).toBe("ready");
});

Then("the option panel reads the final value", async ({ page }) => {
  expect["soft"](page.option).toBe("idle");
});

// Positive control: the factory position already resolves, and must keep resolving. Equal values
// on the same subject are a genuine duplicate, so this case cannot pass by suppressing computed
// access everywhere. The wording is far apart so only `duplicate-handler` applies.
Then("the factory gauge is settled", async ({ page }) => {
  expect(page.factory)["not"].toBe("ready");
});

Then("the archive factory reading has finished", async ({ page }) => {
  expect(page.factory)["not"].toBe("ready");
});
