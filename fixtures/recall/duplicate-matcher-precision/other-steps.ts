// Purpose: matcher identity is not scoped to a file. This matcher is identical to one registered
// in `steps.ts`, and both claim the same step at runtime, so the pair must be reported across the
// file boundary. A regression scoping the comparison class per file would lose it.
import { Given } from "@cucumber/cucumber";

Given("the iota shaft drives", () => iotaInOtherFile());
