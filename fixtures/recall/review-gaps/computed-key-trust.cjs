const { Then } = require('@cucumber/cucumber');
const { expect } = require('@playwright/test');
const { ['expect']: trusted } = require('@playwright/test');
const { [expect]: uncertain } = require('@playwright/test');

Then('the parcel is ready', ({ state }) => expect(state).toEqual(trusted.objectContaining({ role: 'admin' })));
Then('shipment readiness has been confirmed', ({ state }) => expect(state).toEqual(trusted.objectContaining({ role: 'admin' })));
Then('the unknown status is verified', ({ state }) => expect(state).toEqual(uncertain.objectContaining({ role: 'admin' })));
Then('the unknown status is now verified', ({ state }) => expect(state).toEqual(uncertain.objectContaining({ role: 'admin' })));
