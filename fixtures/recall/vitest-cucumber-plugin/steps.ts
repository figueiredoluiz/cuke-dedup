import { Given as givenStep } from 'vitest-cucumber-plugin';
import * as bdd from 'vitest-cucumber-plugin';

givenStep('the parcel is ready', () => expect(parcel).toBe('ready'));
bdd.Given('the parcel is ready', function readyParcel() {
  expect(parcel).toBe('offline');
});
bdd.Given('shipment is ready', () => expect(parcel).toBe('ready'));
bdd.Given('the parcel is idle', () => expect(parcel).toBe('idle'));
