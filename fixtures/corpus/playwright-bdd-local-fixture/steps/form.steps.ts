import { expect, Then } from '~/fixtures/test';

Then('the recently edited field shows its value', async ({ formWorld }) => {
  await expect(formWorld.editor).toHaveValue(formWorld.entered);
});

Then('the added field shows the entered value', async ({ formWorld }) => {
  await expect(formWorld.editor).toHaveValue(formWorld.entered);
});
