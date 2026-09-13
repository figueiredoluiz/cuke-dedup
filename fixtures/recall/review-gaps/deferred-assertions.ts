import { Then } from '@cucumber/cucumber';
import { expect } from '@playwright/test';

Then('the archive badge is visible', ({ state }) => register(() => expect(state.archive).toBe('ready')));
Then('the archive badge is now visible', ({ state }) => otherWrapper(function () { expect(state.archive).toBe('ready'); }));

Then('the parcel status is verified', ({ state }) => register(() => expect(state.parcel).toBe('ready')));
Then('the parcel status is now verified', ({ state }) => register(() => expect(state.parcel).toBe('idle')));

Then('the switch position is checked', ({ state }) => register(() => expect(state.switch).toBe('on')));
Then('the switch position is now checked', ({ state }) => register(() => expect(state.switch).not.toBe('on')));

Then('the warehouse is inspected', () => register(() => { ((value) => save(value))(external); }));
Then('the warehouse is now inspected', () => register(() => { (function (value) { save(value); })(external); }));
