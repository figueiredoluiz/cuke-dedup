import { Then } from "playwright-bdd/decorators";
import { expect } from "@playwright/test";
class ParcelSteps {
  @Then("the parcel status is verified")
  first({ state }) {
    const expected = "ready";
    expect(state).toBe(expected);
  }
  @Then("the parcel status is now verified")
  second({ state }) {
    const expected = "idle";
    expect(state).toBe(expected);
  }
}
