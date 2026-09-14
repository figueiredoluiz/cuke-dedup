// Positive control in its own file, so the mutation above cannot reach it. Without a write the
// alias stays trusted and these identical handlers must still be reported.
const api = require("@playwright/test");
const { Then } = require("@cucumber/cucumber");

const { expect: { not: negated } } = api;
void negated;

Then("the crate label is confirmed", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ role: "editor" }));
});

Then("the crate label is now confirmed", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ role: "editor" }));
});
