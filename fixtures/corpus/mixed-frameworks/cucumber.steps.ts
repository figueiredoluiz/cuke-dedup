import { Given } from "@cucumber/cucumber";

Given("a shared framework registration", async function () {
  await this.openWorkspace();
});
