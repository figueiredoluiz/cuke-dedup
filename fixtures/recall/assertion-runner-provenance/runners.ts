import { Given } from "@cucumber/cucumber";
import { expect as playwrightExpect } from "@playwright/test";
import { expect as vitestExpect } from "vitest";
import { expect as bunExpect } from "bun:test";
import { expect as chaiExpect } from "chai";
import { expect as unknownExpect } from "some-unknown-assertion-lib";

// A recognized runner's factory is trusted, so the expected value is behaviour and two steps
// asserting different values are genuinely different steps rather than one waiting to be
// parameterized. Chain recognition is shape-based, which is why Chai's `.to.equal` needs nothing
// beyond provenance.
Given("the playwright gauge reads a first value", () => playwrightExpect(gauge()).toBe(1));
Given("the playwright gauge reads a second value", () => playwrightExpect(gauge()).toBe(2));

Given("the vitest dial reads a first entry", () => vitestExpect(dial()).toBe(1));
Given("the vitest dial reads a second entry", () => vitestExpect(dial()).toBe(2));

Given("the bun probe reads a first sample", () => bunExpect(probe()).toBe(1));
Given("the bun probe reads a second sample", () => bunExpect(probe()).toBe(2));

Given("the chai sensor reads a first reading", () => chaiExpect(sensor()).to.equal(1));
Given("the chai sensor reads a second reading", () => chaiExpect(sensor()).to.equal(2));

// An unrecognized module is not trusted: its call arguments are not assertions, so the pair differs
// only in an unproven value. Declaring the module through `assertionModules` is the supported way
// to restore trust.
Given("the unknown valve reads a first state", () => unknownExpect(valve()).toBe(1));
Given("the unknown valve reads a second state", () => unknownExpect(valve()).toBe(2));
