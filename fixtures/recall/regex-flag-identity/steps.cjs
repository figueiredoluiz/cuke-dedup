// Flags that change what a matcher matches belong to its identity; flags that only change how a
// match is executed do not. `v` enables Unicode set notation and changes character-class
// semantics, so it is at least as significant as `u`, which it implies.
const { Given } = require("@cucumber/cucumber");

// `v` is semantic: these two match different inputs and must not be reported as equivalent.
Given(/^the pallet gauge is settled$/v, () => {
  workUnicodeSets();
});

Given(/^the pallet gauge is settled$/, () => {
  workPlain();
});

// `g` only changes how a match is executed, so these two remain the same matcher. This is the
// control: if every flag were made semantic, this finding would disappear.
Given(/^the crate gauge is settled$/g, () => {
  workGlobal();
});

Given(/^the crate gauge is settled$/, () => {
  workOnce();
});
