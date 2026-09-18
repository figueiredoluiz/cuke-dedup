// Both Cypress entrypoints register; the legacy root package exports a preprocessor and must not.
const { Given } = require("@badeball/cypress-cucumber-preprocessor");
const legacySteps = require("cypress-cucumber-preprocessor/steps");

Given("the nu piston strokes {word}", () => n1());
Given("the nu piston strokes long", () => n2());

legacySteps.Given("the xi bearing rolls {word}", () => o1());
legacySteps.Given("the xi bearing rolls freely", () => o2());
