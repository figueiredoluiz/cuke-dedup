import { Given, Then } from "playwright-bdd/decorators";

export class DashboardSteps {
  @Given("an account with dashboard access")
  async prepare({ page: firstPage }) {
    await firstPage.goto("/dashboard");
  }

  @Then("the dashboard can be opened", { timeout: 1000 })
  async verify({ page: secondPage }) {
    await secondPage.goto("/dashboard");
  }
}
