const { Given } = require("@cucumber/cucumber");
const fixtures = require("./fixtures");
Given("the delta sensor logs a first sample", () => fixtures.expect(sensor()).toBe(1));
Given("the delta sensor logs a second sample", () => fixtures.expect(sensor()).toBe(2));
