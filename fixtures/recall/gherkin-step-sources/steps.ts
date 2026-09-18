import { Given, Then } from "@cucumber/cucumber";

// Purpose: prove which Gherkin constructs contribute concrete steps, using `unused-definition` as
// the observable. A definition reachable only through one construct must NOT be reported unused; if
// that construct stopped contributing steps, the finding would appear. The last definition is the
// positive control that keeps the rule demonstrably live.

// Reachable only from a `Background` step.
Given("the site is reachable", () => reachable());

// Reachable only by expanding a `Scenario Outline` row from `Examples`.
Given("the {word} queue is drained", () => drained());

// Reachable only through a `Then` continuation inside the outline.
Then("the audit trail is written", () => audited());

// Reachable only through an `And` continuation in a plain scenario.
Given("the report is archived", () => archived());

// Matched by nothing: the positive control.
Given("the vault is sealed", () => sealed());
