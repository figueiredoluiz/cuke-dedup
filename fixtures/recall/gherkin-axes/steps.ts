import { Given } from "@cucumber/cucumber";

// Each selected feature makes exactly one definition used; the other three remain positive controls.
Given("the shared setup is ready", () => setup());
Given("the queue {word} is ready", () => queue());
Given("the continuation is verified", () => continuation());
Given("the second file is visible", () => secondFile());
