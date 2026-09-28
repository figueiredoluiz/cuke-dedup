import { Given } from "@cucumber/cucumber";

// Purpose: one warning stays visible while a separate finding is suppressed in structured reports.
Given("the active pair runs", () => first());
Given("the active pair runs", () => second());
// cuke-dedup:ignore duplicate-matcher -- intentional alternate binding
Given("the suppressed pair runs", () => third());
Given("the suppressed pair runs", () => fourth());
