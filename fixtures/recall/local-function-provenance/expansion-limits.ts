import { Given } from "@cucumber/cucumber";

// Declared and never called: nothing executes, so the values are not behaviour. The pair differs
// only in an unproven value.
Given("the crane idles at a first angle", () => { /** Declared, never called. */ function act() { expect(crane()).toBe(1); } });
Given("the crane idles at a second angle", () => { /** Declared, never called. */ function act() { expect(crane()).toBe(2); } });

// Arguments are not substituted at the call, so a parameterized local function's body is unproven.
// The two handlers are identical on purpose: the pair is silent only because that body is unproven
// and the pair is excluded from comparison. Were the body expanded, identical handlers would be
// reported as duplicates, so a distinct argument here would hide that regression.
Given("the pulley turns to a first notch", () => {
  /** Parameterized, so its body is unproven at the call. */
  function act(value) { expect(pulley()).toBe(value); }
  act(1);
});
Given("the pulley turns to a second notch", () => {
  /** Parameterized, so its body is unproven at the call. */
  function act(value) { expect(pulley()).toBe(value); }
  act(1);
});

// Expansion is single level, so self-recursion terminates rather than walking forever.
Given("the spindle cycles a first turn", () => {
  function act() { expect(spindle()).toBe(1); act(); }
  act();
});
Given("the spindle cycles a second turn", () => {
  function act() { expect(spindle()).toBe(2); act(); }
  act();
});

// A declaration nested in another function is not visible at this call site.
Given("the gantry shifts a first span", () => {
  function outer() { function act() { expect(gantry()).toBe(1); } }
  act();
});
Given("the gantry shifts a second span", () => {
  function outer() { function act() { expect(gantry()).toBe(2); } }
  act();
});

// `enum` binds its name at runtime exactly as `class` does; the inverted shadow rule covers both
// without enumerating either.
Given("the ratchet holds a first tooth", () => {
  function act() { expect(ratchet()).toBe(1); }
  { enum act { A } act(); }
});
Given("the ratchet holds a second tooth", () => {
  function act() { expect(ratchet()).toBe(2); }
  { enum act { A } act(); }
});
