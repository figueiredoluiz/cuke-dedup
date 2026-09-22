const { Given } = require("@cucumber/cucumber");

// Proven module constants: two handlers reading the same value through differently named constants
// are the same handler. Renaming cannot express this, so the value itself reaches the fingerprint.
const ALPHA_LIMIT = 1;
const ALPHA_BOUND = 1;
Given("the alpha meter reaches its limit", () => expect(meter()).toBe(ALPHA_LIMIT));
Given("the alpha meter reaches its bound", () => expect(meter()).toBe(ALPHA_BOUND));

// Conflicting values stay distinct, exactly as the literal spelling does.
const BETA_FIRST = 1;
const BETA_SECOND = 2;
Given("the beta dial holds its first stop", () => expect(dial()).toBe(BETA_FIRST));
Given("the beta dial holds its second stop", () => expect(dial()).toBe(BETA_SECOND));

// `let` can be reassigned between registration and execution, so the value is never proven.
let gammaFirst = 1;
let gammaSecond = 1;
Given("the gamma probe tracks a mutable first point", () => expect(probe()).toBe(gammaFirst));
Given("the gamma probe tracks a mutable second point", () => expect(probe()).toBe(gammaSecond));

// Control for the shape above: genuinely module-scoped values with no assertion must still collapse.
const TOP_ONE = 1;
const TOP_TWO = 1;
Given("the top slot is pressed first", () => clickButton(TOP_ONE));
Given("the top slot is pressed second", () => clickButton(TOP_TWO));

// A string value resolves exactly as a number does.
const ALPHA_LABEL = "ready";
const ALPHA_TAG = "ready";
Given("the alpha panel shows its label", () => expect(panel()).toBe(ALPHA_LABEL));
Given("the alpha panel shows its tag", () => expect(panel()).toBe(ALPHA_TAG));

// A handler-local binding shadows the module constant and keeps its own value.
const SHARED_STOP = 1;
Given("the omega brake holds a local stop", () => { const SHARED_STOP = 2; expect(brake()).toBe(SHARED_STOP); });
Given("the omega brake holds a module stop", () => expect(brake()).toBe(SHARED_STOP));

// A module constant declared *after* the registrations that read it still resolves: a handler is a
// deferred callback, so the module has finished initializing by the time it runs. Same value
// collapses just as a declared-before pair does.
Given("the delta valve settles at its early mark", () => expect(valve()).toBe(DELTA_EARLY));
Given("the delta valve settles at its late mark", () => expect(valve()).toBe(DELTA_LATE));
const DELTA_EARLY = 1;
const DELTA_LATE = 1;

// Conflicting later-declared values stay distinct, exactly as declared-before conflicts do.
Given("the epsilon lever locks at its low notch", () => expect(lever()).toBe(EPSILON_LOW));
Given("the epsilon lever locks at its high notch", () => expect(lever()).toBe(EPSILON_HIGH));
const EPSILON_LOW = 1;
const EPSILON_HIGH = 2;
