const { Given } = require("@cucumber/cucumber");

// Called local function: its assertions are the handler's behaviour, so identical bodies are one
// handler and conflicting expected values keep the handlers apart.
Given("the alpha meter settles evenly", () => { function check() { expect(meter()).toBe(1); } check(); });
Given("the alpha meter settles smoothly", () => { function check() { expect(meter()).toBe(1); } check(); });

Given("the beta dial resolves upward", () => { function check() { expect(dial()).toBe(1); } check(); });
Given("the beta dial resolves downward", () => { function check() { expect(dial()).toBe(2); } check(); });
