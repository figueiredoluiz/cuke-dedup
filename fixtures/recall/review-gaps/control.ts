import { Then } from "@cucumber/cucumber";
import * as api from "@playwright/test";
Then("the parcel status is now verified", ({ state }) => api.expect(state).toBe("ready"));
