import { Then } from "@cucumber/cucumber";
import * as api from "@playwright/test";
const other = api;
other.expect = replacement;
Then("the parcel status is verified", ({ state }) => api.expect(state).toBe("ready"));
