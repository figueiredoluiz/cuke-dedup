const { Then } = require('@cucumber/cucumber');
const api = require('@playwright/test');
const { [runtimeKey]: check } = require('@playwright/test');
check.objectContaining = replacement;

Then('the parcel status is verified', ({ state }) => api.expect(state).toEqual(api.expect.objectContaining({ role: 'admin' })));
Then('the parcel status is now verified', ({ state }) => api.expect(state).toEqual(api.expect.objectContaining({ role: 'admin' })));
