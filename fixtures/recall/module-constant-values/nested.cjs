const { Given } = require("@cucumber/cucumber");

// Constants declared inside another declaration's initializer are not module scope. They must never
// substitute: the handlers below read unresolved module references, not a shared value.
const helper = () => {
  const DELTA_ONE = 1;
  const DELTA_TWO = 1;
  const DEEP_ONE = 1;
  const DEEP_TWO = 1;
  return [DELTA_ONE, DELTA_TWO, DEEP_ONE, DEEP_TWO];
};
Given("the delta sensor samples a nested first slot", () => expect(sensor()).toBe(DELTA_ONE));
Given("the delta sensor samples a nested second slot", () => expect(sensor()).toBe(DELTA_TWO));

// Without an assertion the behaviour signatures match, so the alpha fingerprint alone decides. This
// is the shape where a missing scope check becomes visible: substituting these nested values would
// make two unrelated handlers one handler.
Given("the nested slot is pressed first", () => clickButton(DEEP_ONE));
Given("the nested slot is pressed second", () => clickButton(DEEP_TWO));

// A handler can close over a binding in an enclosing scope. That name is neither handler-local nor
// module-scoped, so a module constant of the same name must not substitute over the captured value.
// Doing so made these two handlers comparable and equal, which is a false duplicate: they assert
// different values at runtime.
const CAPTURED_LIMIT = 1;
function buildFirst() {
  const CAPTURED_LIMIT = 7;
  Given("the capstan winds to a first limit", () => expect(capstan()).toBe(CAPTURED_LIMIT));
}
function buildSecond() {
  const CAPTURED_LIMIT = 8;
  Given("the capstan winds to a second limit", () => expect(capstan()).toBe(CAPTURED_LIMIT));
}
buildFirst();
buildSecond();

// A captured *parameter* is the sibling of the captured declaration above. Recording only one form
// left the other as a latent false duplicate, so both are pinned here.
const CAPTURED_SPAN = 1;
function spanFirst(CAPTURED_SPAN) {
  Given("the derrick swings to a first span", () => expect(derrick()).toBe(CAPTURED_SPAN));
}
function spanSecond(CAPTURED_SPAN) {
  Given("the derrick swings to a second span", () => expect(derrick()).toBe(CAPTURED_SPAN));
}
spanFirst(7);
spanSecond(8);
