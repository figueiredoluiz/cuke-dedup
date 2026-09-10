import { Given } from "@cucumber/cucumber";

Given("I am on login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
});

Given("I am on the login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
  await this.page.waitForLoadState();
});
