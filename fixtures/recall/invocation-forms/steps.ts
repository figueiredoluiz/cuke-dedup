import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";

// A parameterized immediately-invoked function keeps the ordinary call boundary because its
// arguments are never substituted. The handler must stay comparable regardless, so two identical
// implementations behind distinct wording remain a duplicate.
Then("the summary panel is ready", async ({ page }) => {
  ((target) => {
    expect(target.status).toBe("ready");
  })(page);
});

Then("the inventory badge has finished loading", async ({ page }) => {
  ((target) => {
    expect(target.status).toBe("ready");
  })(page);
});

// A zero-parameter immediately-invoked function executes inline, so its assertion is visible and
// conflicting expected values must keep close wording apart.
Then("the detail panel shows the first state", async ({ page }) => {
  (() => {
    expect(page.status).toBe("ready");
  })();
});

Then("the detail panel shows the final state", async ({ page }) => {
  (() => {
    expect(page.status).toBe("idle");
  })();
});
