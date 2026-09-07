import { defineBddConfig } from 'playwright-bdd';

export const testDir = defineBddConfig({
  features: ['specs/**/*.spec'],
  steps: ['steps.ts'],
});
