// Object rest copies property values into a fresh object. Replacing a slot on that copy changes
// nothing the source can observe, so trust must survive and these duplicates must still be found.
// Treating the write as a source mutation would silence every assertion the module reaches, not
// just this pair.
const api = require("@playwright/test");
const { Then } = require("@cucumber/cucumber");

const { expect: { ...copy } } = api;
copy.not = replacement;

Then("the pallet label is confirmed", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ tier: "gold" }));
});

Then("the pallet label is now confirmed", ({ state }) => {
  api.expect(state).toEqual(api.expect.not.objectContaining({ tier: "gold" }));
});
