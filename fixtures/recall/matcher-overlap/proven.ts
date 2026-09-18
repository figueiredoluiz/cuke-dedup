import { Given } from "@cucumber/cucumber";

// A discovered feature step proves the ambiguity, so `ambiguous-step` owns this pair and overlap is
// not reported alongside it. Overlap is for pairs no feature exercises.
Given("the eta hatch opens {word}", () => etaOne());
Given("the eta hatch opens wide", () => etaTwo());
