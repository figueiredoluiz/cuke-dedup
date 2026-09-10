import { Given } from "@cucumber/cucumber";

Given("I am on login page", async function () {
  await this.page.goto("/login");
});

Given("I am on the login page", async function () {
  await this.page.goto("/login");
  await this.page.waitForLoadState();
});
