import { Then } from "@cucumber/cucumber";
import { expect as check } from "@playwright/test";
import { Given } from "playwright-bdd/decorators";

Then("the settings panel shows the primary account field", async ({ page }, expected) => {
  await expect(new AccountForm(page).primaryInput).toHaveValue(expected);
});

Then("the settings panel shows the secondary account field", async ({ page }, expected) => {
  await expect(new AccountForm(page).secondaryInput).toHaveValue(expected);
});

Then("the navigation drawer state indicator is collapsed", async ({ page }) => {
  await expect(new Navigation(page).drawer).toHaveClass(/collapsed/);
});

Then("the navigation drawer state indicator is expanded", async ({ page }) => {
  await expect(new Navigation(page).drawer).not.toHaveClass(/collapsed/);
});

Then("the account badge is visible", async ({ page }) => {
  await expect(new AccountPage(page).badge).toBeVisible();
});

Then("the account badge is now visible", async ({ page }) =>
  expect(new AccountPage(page).badge).toBeVisible(),
);

Then("the account status indicator shows the first condition", async ({ page }) => {
  await expect(new AccountPage(page).status).toBe("ready");
});

Then("the account status indicator shows the final condition", async ({ page }) => {
  await expect(new AccountPage(page).status).toBe("idle");
});

Then("the profile state indicator shows the first condition", async ({ page }) => {
  await check(new ProfilePage(page).state).toBe("ready");
});

Then("the profile state indicator shows the final condition", async ({ page }) => {
  await check(new ProfilePage(page).state).toBe("idle");
});

class DecoratedStatusSteps {
  @Given("the decorated status shows the first condition")
  first({ page }) {
    check(page.status).toBe("ready");
  }

  @Given("the decorated status shows the final condition")
  final({ page }) {
    check(page.status).toBe("idle");
  }
}
