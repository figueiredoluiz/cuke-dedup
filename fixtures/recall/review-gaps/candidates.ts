import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel status is verified", ({ state }) => { expect(state).toBe("ready"); });
Then("the parcel status is now verified", ({ state }) => { expect(state).toBe("idle"); });
Then("the archive badge is visible", ({ page }) => { expect(page.badge).toBeVisible(); });
Then("the archive badge is now visible", ({ page }) => expect(page.badge).toBeVisible());
