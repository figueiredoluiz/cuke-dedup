// Purpose: `normalized-matcher` fires when matchers become equal after Unicode, whitespace,
// placeholder and regular-expression normalization, and must NOT fire across distinct matcher kinds
// or differing effective regex flags (docs/rules.md). One pair per axis, one per exclusion.
//
// Flag *order* is deliberately not here: the effective flag set is a set, so `/x/im` against
// `/x/mi` is an exact duplicate and belongs to `duplicate-matcher-precision`.
//
// Every handler is distinct, so a finding can only come from a matcher rule.
import { Given } from "@cucumber/cucumber";

// Axis 1 — whitespace: runs of spaces collapse.
Given("the alpha gate  opens", () => alphaFirst());
Given("the alpha gate opens", () => alphaSecond());

// Axis 2 — Unicode: NFKC folds composed and decomposed forms together. Byte-different, visually
// identical: `\u00e9` against `e\u0301`.
Given("the café counter serves", () => unicodeFirst());
Given("the café counter serves", () => unicodeSecond());

// Axis 3 — placeholder: whitespace inside a Cucumber Expression placeholder is trimmed, so
// `{ int }` and `{int}` are the same parameter.
Given("the gamma dial reads { int }", () => gammaFirst());
Given("the gamma dial reads {int}", () => gammaSecond());

// Axis 4 — regular expression: a capture group's *name* is not part of what the matcher accepts.
Given(/the delta pump runs (?<first>\d+)/, () => deltaFirst());
Given(/the delta pump runs (?<second>\d+)/, () => deltaSecond());

// Exclusion 1 — distinct kinds stay distinct even though the texts normalize alike. `matcher_kind`
// is part of the normalized comparison class (pairs.rs); the `[regex-flags:...]` prefix a regular
// expression's normalized form carries is additional separation, not the only one.
Given("the epsilon lamp  glows", () => epsilonFirst());
Given(/the epsilon lamp glows/, () => epsilonSecond());

// Exclusion 2 — an effective flag stays distinct even though the texts normalize alike.
Given(/the zeta rotor  spins/, () => zetaFirst());
Given(/the zeta rotor spins/i, () => zetaSecond());

// Exclusion 3 — placeholder normalization renames, it does not erase: `{int}` and `{float}`
// accept different text.
Given("the eta brake holds {int}", () => etaFirst());
Given("the eta brake holds {float}", () => etaSecond());

// Exclusion 4 — capture-group normalization preserves how many groups there are.
Given(/the theta clamp grips (\d+) at (\w+)/, () => thetaFirst());
Given(/the theta clamp grips (\d+) at \w+/, () => thetaSecond());

// Axis 5 — Unicode compatibility, which is not the same folding as axis 2. Axis 2 pairs canonical
// composition (NFC against NFD, same abstract characters); this pairs a full-width code point with
// its ASCII equivalent, which only NFKC's compatibility mapping folds together.
Given("the ｉota shaft drives", () => iotaFirst());
Given("the iota shaft drives", () => iotaSecond());

// Axis 6 — regular-expression whitespace: an escaped space and a literal space accept the same
// text. Distinct from axis 4, which is about capture-group naming.
Given(/the kappa gear\ meshes/, () => kappaFirst());
Given(/the kappa gear meshes/, () => kappaSecond());

// Exclusion 5 — anchors change what the matcher accepts, so normalization must not drop them.
Given(/^the lambda seal closes$/, () => lambdaFirst());
Given(/the lambda seal closes/, () => lambdaSecond());

// Exclusion 6 — quantifiers change what the matcher accepts.
Given(/the mu spring loads+/, () => muFirst());
Given(/the mu spring loads/, () => muSecond());
