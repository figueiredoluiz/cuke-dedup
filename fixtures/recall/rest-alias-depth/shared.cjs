// The same copy, written one level deeper. `copy.not` is the object `api.expect.not` also holds,
// so this mutates the matcher the handlers below rely on and trust must be revoked.
const api = require("@playwright/test");
const { Then } = require("@cucumber/cucumber");

const { ...clone } = api.expect;
clone.not.objectContaining = replacement;

Then("the crate seal is verified", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ tier: "gold" }));
});

Then("the crate seal is now verified", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ tier: "gold" }));
});
