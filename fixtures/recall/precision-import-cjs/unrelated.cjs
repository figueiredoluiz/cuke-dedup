const { Then } = require("@cucumber/cucumber");
const { expect } = require("synthetic-assertion-library");
Then("the parcel status is now verified", ({ state }) => expect(state).toBe("ready"));
