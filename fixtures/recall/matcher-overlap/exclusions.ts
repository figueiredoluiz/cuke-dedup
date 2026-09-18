import { Given } from "@cucumber/cucumber";

// Equivalent matchers are already connected by the duplicate rules, so overlap adds nothing and
// must not be reported a second time.
Given("the delta valve is sealed", () => deltaOne());
Given("the delta valve is sealed", () => deltaTwo());

// An unsupported regular expression is never reversed into a sample: no witness exists for a regex
// matcher, so an overlapping pair of regexes yields no overlap finding.
Given(/^the epsilon pump runs .+$/, () => epsilonOne());
Given(/^the epsilon pump runs fast$/, () => epsilonTwo());

// An undeclared parameter type has no soundly derivable sample, so it contributes no witness.
Given("the zeta lamp turns {colour}", () => zetaOne());
Given("the zeta lamp turns red", () => zetaTwo());
