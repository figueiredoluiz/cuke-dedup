import { createBdd } from "playwright-bdd";

const { Given: Setup } = createBdd();
Setup("a shared framework registration", async ({ page }) => {
  await page.goto("/workspace");
});
