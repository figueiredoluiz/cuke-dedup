import { Given } from "@cucumber/cucumber";

// Purpose: the outline step expands to "the <a7 value". The first definition matches that
// expansion and must be used; the second matches the unexpanded text and must be reported unused.
Given("the <a7 value", () => expanded());
Given("the <a<b> value", () => verbatim());
