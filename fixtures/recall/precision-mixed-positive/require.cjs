const { Then } = require("@cucumber/cucumber");
const { expect } = require("@playwright/test");
Then("shipment readiness has been confirmed", ({ state }) => expect(state).toBe("ready"));
