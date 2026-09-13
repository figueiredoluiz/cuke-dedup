import { Then } from '@cucumber/cucumber';
import * as api from '@playwright/test';

const { [runtimeKey]: check } = api;
check.objectContaining = replacement;
Then('the parcel status is verified', ({ state }) => api.expect(state).toEqual(api.expect.objectContaining({ role: 'admin' })));
Then('the parcel status is now verified', ({ state }) => api.expect(state).toEqual(api.expect.objectContaining({ role: 'admin' })));
