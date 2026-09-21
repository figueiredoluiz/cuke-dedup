// Purpose: pin the documented boundary between a spanning set of pair findings and a single
// bounded cluster finding (docs/rules.md, "Exact groups and fuzzy pairs"): groups of up to four
// definitions produce n-1 pair findings; larger groups produce one cluster carrying the complete
// member count. Duplication counts every member either way.
//
// Nothing in either corpus pinned this before, so a change collapsing four-member groups into a
// cluster — or expanding five-member clusters back into ten pair findings — would have shipped
// silently. The truncation half of that contract needs more members than the report cap and is
// unit-tested against the `cfg(test)` cap instead, so it is deliberately not duplicated here.
//
// Every handler is distinct, so only the matcher rule can speak.
import { Given } from "@cucumber/cucumber";

// Four members: the largest group that still reports as a spanning set of pairs.
Given("the alpha gate opens", () => alpha1());
Given("the alpha gate opens", () => alpha2());
Given("the alpha gate opens", () => alpha3());
Given("the alpha gate opens", () => alpha4());

// Five members: one past the boundary, so this collapses into a single cluster finding.
Given("the beta valve turns", () => beta1());
Given("the beta valve turns", () => beta2());
Given("the beta valve turns", () => beta3());
Given("the beta valve turns", () => beta4());
Given("the beta valve turns", () => beta5());

// A distinct property of the same machinery: cluster membership follows the *effective* matcher, so
// cosmetic flag variation must not fragment one group into several. All five below are the same
// effective matcher (`g`, `y` and their orderings are outside the semantic set), so they must form
// one five-member cluster rather than a scatter of pair findings. Candidate grouping still keys on
// the raw flag string (pairs.rs), and this is what proves the reconnection keeps the group whole.
Given(/^the gamma dial reads$/, () => gamma1());
Given(/^the gamma dial reads$/g, () => gamma2());
Given(/^the gamma dial reads$/y, () => gamma3());
Given(/^the gamma dial reads$/gy, () => gamma4());
Given(/^the gamma dial reads$/yg, () => gamma5());

// Three members: mid-range, so the spanning set is confirmed as n-1 findings across the whole
// permitted range rather than only at its upper endpoint.
Given("the delta pump runs", () => delta1());
Given("the delta pump runs", () => delta2());
Given("the delta pump runs", () => delta3());
