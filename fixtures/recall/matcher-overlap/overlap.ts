import { Given } from "@cucumber/cucumber";

// `overlapping-matcher` synthesizes a concrete Cucumber Expression accepted by two definitions and
// reports the pair even though no discovered feature exercises it. The witness for `{word}` is
// `sample`, for `{int}` it is `1`.
Given("the alpha gauge shows {word}", () => alphaOne());
Given("the alpha gauge shows sample", () => alphaTwo());

// The witness is matched against regular-expression matchers too, not only expressions.
Given("the beta dial reads {word}", () => betaOne());
Given(/^the beta dial reads sample$/, () => betaTwo());

Given("the gamma meter counts {int}", () => gammaOne());
Given("the gamma meter counts 1", () => gammaTwo());
