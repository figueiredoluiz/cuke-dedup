import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel status is verified", ({ state }) => {
  const expected = "ready";
  { expect(state).toBe(expected); const expected = "idle"; }
});
Then("the parcel status is now verified", ({ state }) => {
  const expected = "ready";
  { expect(state).toBe(expected); const expected = "ready"; }
});
