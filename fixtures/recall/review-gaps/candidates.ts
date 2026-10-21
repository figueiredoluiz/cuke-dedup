import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel status is verified", ({ state }) => { expect(state).toBe("ready"); });
Then("the parcel status is now verified", ({ state }) => { expect(state).toBe("idle"); });
Then("the archive badge is visible", ({ page }) => { expect(page.badge).toBeVisible(); });
Then("the archive badge is now visible", ({ page }) => expect(page.badge).toBeVisible());

// The parcel pair differs in a value, so its behavior signatures differ and no candidate source
// proposes it: without a limit the run skips nothing and the pair has no record. Under a candidate
// limit matcher blocking charges a skip before it drops the pair, so the run reports a skipped
// comparison and the pair still has no record. The manifest pins the line numbers above.
