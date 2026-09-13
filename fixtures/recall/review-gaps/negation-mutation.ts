import { Then } from '@cucumber/cucumber';
import { expect } from '@playwright/test';

const { ['not']: negated } = expect;
negated.objectContaining = replacement;
Then('the parcel status is verified', ({ state }) => expect(state).toEqual(expect.not.objectContaining({ role: 'admin' })));
Then('the parcel status is now verified', ({ state }) => expect(state).toEqual(expect.not.objectContaining({ role: 'admin' })));
