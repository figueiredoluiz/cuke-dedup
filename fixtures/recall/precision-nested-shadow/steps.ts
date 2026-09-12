import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel status is verified", ({ state }) => {
  function inspect(expect) { return expect("unrelated"); }
  expect(state).toBe("ready");
});
Then("the parcel status is now verified", ({ state }) => {
  function inspect(expect) { return expect("unrelated"); }
  expect(state).toBe("idle");
});
