import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
const expected = "idle";
Then("the parcel status is now verified", ({ state }) => expect(state).toBe(expected));
