import { Given } from 'vitest-cucumber-plugin';

Given('plugin parcel is ready', () => expect(parcel).toBe('ready'));
// Of the identical scenario-local handlers in steps.test.ts only the first has a pair record with
// this definition: the identical-handler source proposes a group's anchor against the other matcher
// groups. That record shows the analyzer judging the handlers runtime-incompatible (jest-cucumber's
// scenario-local handlers against the Vitest plugin's global ones), which is what the cross-framework
// negatives rely on.
