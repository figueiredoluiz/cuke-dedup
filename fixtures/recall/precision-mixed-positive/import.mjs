import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel is ready", ({ state }) => expect(state).toBe("ready"));
