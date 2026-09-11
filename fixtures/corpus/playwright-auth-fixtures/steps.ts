import { createBdd } from "playwright-bdd";

const { Given, Then } = createBdd();

Given("an authenticated session exists", async ({ page: adminPage }) => {
  await adminPage.goto("/secure");
});

Then("the protected dashboard is ready", async ({ page: memberPage }) => {
  await memberPage.goto("/secure");
});
