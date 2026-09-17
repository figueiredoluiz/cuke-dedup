const { Given } = require("@cucumber/cucumber");

// A variable binding shadows the declaration, so the outer body must not be attributed here.
Given("the gamma probe reads a shadowed first value", () => {
  function check() { expect(probe()).toBe(9); }
  { const check = () => { expect(probe()).toBe(1); }; check(); }
});
Given("the gamma probe reads a shadowed second value", () => {
  function check() { expect(probe()).toBe(9); }
  { const check = () => { expect(probe()).toBe(2); }; check(); }
});

// Reassignment replaces the body before the call, so the declaration no longer describes what runs.
Given("the delta sensor logs a replaced first sample", () => {
  function check() { expect(sensor()).toBe(9); }
  check = () => { expect(sensor()).toBe(1); };
  check();
});
Given("the delta sensor logs a replaced second sample", () => {
  function check() { expect(sensor()).toBe(9); }
  check = () => { expect(sensor()).toBe(2); };
  check();
});

// Discriminating shape for reassignment: the declarations differ but the replacement bodies are
// identical. If the declaration were expanded, its conflicting values would separate the handlers
// and this finding would disappear.
Given("the relay trips on a first path", () => {
  function act() { expect(relay()).toBe(1); }
  act = () => { expect(relay()).toBe(9); };
  act();
});
Given("the relay trips on a second path", () => {
  function act() { expect(relay()).toBe(2); }
  act = () => { expect(relay()).toBe(9); };
  act();
});
