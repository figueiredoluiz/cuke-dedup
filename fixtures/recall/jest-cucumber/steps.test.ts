import { defineFeature, loadFeature } from 'jest-cucumber';

const feature = loadFeature('./shipment.feature');

defineFeature(feature, test => {
  test('Ready parcel', ({ given }) => {
    given('the parcel is ready', () => expect(parcel).toBe('ready'));
  });

  test('Ready parcel again', ({ given }) => {
    given('the parcel is ready', () => expect(parcel).toBe('ready'));
  });

  test('Ready parcel, different wording', ({ given }) => {
    given('shipment is ready', () => expect(parcel).toBe('ready'));
  });

  test('Idle parcel', ({ given }) => {
    given('the parcel is idle', function idleParcel() {
      expect(parcel).toBe('idle');
    });
  });
});
