// OPEN-1 / R5190030268-S1.
//
// A nested destructuring pattern aliases the same module object as `api.expect.not`, so writing
// through it must revoke assertion trust exactly as the flat alias does. Both handlers are
// identical, so retained trust reports them as duplicates.
const api = require("@playwright/test");
const { Then } = require("@cucumber/cucumber");

const { expect: { not: negated } } = api;
negated.objectContaining = replacement;

Then("the parcel status is verified", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ role: "admin" }));
});

Then("the parcel status is now verified", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ role: "admin" }));
});
