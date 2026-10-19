import { Then } from '@cucumber/cucumber';
import { expect } from '@playwright/test';

Then('the archive badge is visible', ({ state }) => register({ callback: () => expect(state.archive).toBe('ready') }));
Then('the archive badge is now visible', ({ state }) => otherWrapper([function () { expect(state.archive).toBe('ready'); }]));

Then('the parcel status is verified', ({ state }) => register({ callback: () => expect(state.parcel).toBe('ready') }));
Then('the parcel status is now verified', ({ state }) => register({ callback: () => expect(state.parcel).toBe('idle') }));

Then('the switch position is checked', ({ state }) => register(() => expect(state.switch).toBe('on')));
Then('the switch position is now checked', ({ state }) => register(() => expect(state.switch).not.toBe('on')));

Then('the warehouse is inspected', () => register(() => { ((value) => save(value))(external); }));
// Identical on purpose: an unresolved inline call keeps the pair out of comparison, and were it
// resolved these identical handlers would be reported as duplicates.
Then('the warehouse is now inspected', () => register(() => { ((value) => save(value))(external); }));
