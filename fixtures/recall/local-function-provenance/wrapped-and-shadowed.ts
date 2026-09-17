import { Given } from "@cucumber/cucumber";

// A transparent wrapper around the callee resolves the same declaration a bare call does, so the
// expansion must reach it. If it did not, these conflicting expected values would stop being
// behaviour and the pair would surface as a parameterization candidate.
Given("the hoist lifts along a first track", () => {
  function act() { expect(hoist()).toBe(1); }
  (act)();
});
Given("the hoist lifts along a second track", () => {
  function act() { expect(hoist()).toBe(2); }
  (act)();
});

// A class binds its name at runtime and shadows the declaration inside the block, so the outer body
// must not be attributed here. Expanding it would read 1 and 2 as conflicting behaviour and remove
// this finding.
Given("the winch spools past a first mark", () => {
  function act() { expect(winch()).toBe(1); }
  { class act {} act(); }
});
Given("the winch spools past a second mark", () => {
  function act() { expect(winch()).toBe(2); }
  { class act {} act(); }
});
