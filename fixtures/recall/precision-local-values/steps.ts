import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel status is verified", ({ state }) => {
  const expected = "ready";
  expect(state).toBe(expected);
});
Then("the parcel status is now verified", ({ state }) => {
  const expected = "idle";
  expect(state).toBe(expected);
});
