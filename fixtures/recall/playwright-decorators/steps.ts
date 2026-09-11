import { Given } from "playwright-bdd/decorators";

export class WorkspaceSteps {
  @Given("decorated workspace registration")
  async prepare() {
    await seedWorkspace();
  }

  @Given("decorated workspace registration", { timeout: 1000 })
  async restore() {
    await restoreWorkspace();
  }
}
