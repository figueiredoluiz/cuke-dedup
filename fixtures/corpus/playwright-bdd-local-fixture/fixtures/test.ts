import { expect } from '@playwright/test';
import { createBdd } from 'playwright-bdd';

const test = {};

export const { Given, When, Then } = createBdd(test);
export { expect };
