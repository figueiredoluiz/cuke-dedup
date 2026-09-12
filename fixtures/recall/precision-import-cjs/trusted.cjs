const { Then } = require("@cucumber/cucumber");
const { expect } = require("@playwright/test");
Then("the parcel status is verified", ({ state }) => expect(state).toBe("ready"));
