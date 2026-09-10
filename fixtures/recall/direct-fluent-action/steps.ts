import { When } from "@cucumber/cucumber";

When("I click the save button", async ({ page }) => {
  await page.click("#save");
});

When("I click save button", async ({ page }) => {
  await page.locator("#save").click();
});
