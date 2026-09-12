import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel status is verified", ({ state }) => (expect)(state).toBe("ready"));
Then("the parcel status is now verified", ({ state }) => (expect as any)(state).toBe("idle"));
Then("the archive badge is visible", ({ state }) => (expect!)(state).toBeVisible());
Then("the archive badge is now visible", ({ state }) => expect(state).toBeVisible());
