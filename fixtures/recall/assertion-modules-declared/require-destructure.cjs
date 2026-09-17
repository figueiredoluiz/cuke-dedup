const { Given } = require("@cucumber/cucumber");
const { expect } = require("./fixtures");
Given("the epsilon valve reports a first state", () => expect(valve()).toBe(1));
Given("the epsilon valve reports a second state", () => expect(valve()).toBe(2));
