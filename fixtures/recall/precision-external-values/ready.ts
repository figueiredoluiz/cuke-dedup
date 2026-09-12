import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
const expected = "ready";
Then("the parcel status is verified", ({ state }) => expect(state).toBe(expected));
