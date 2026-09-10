import { Then } from "@cucumber/cucumber";

Then("the account is enabled", async () => verifyAccount());
Then("the account is not enabled", async () => verifyAccount());
