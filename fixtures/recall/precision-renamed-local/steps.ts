import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel is ready", ({ state }) => {
  const expected = "ready";
  expect(state).toBe(expected);
});
Then("shipment readiness has been confirmed", ({ state }) => {
  const desired = "ready";
  expect(state).toBe(desired);
});
