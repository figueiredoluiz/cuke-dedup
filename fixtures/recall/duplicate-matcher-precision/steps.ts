// Purpose: `duplicate-matcher` requires the same matcher kind, source text AND effective regex
// flags (docs/rules.md). Each silent pair below removes exactly one of those three; each reported
// pair shows something that does NOT change matcher identity. The rule ships at error severity, so
// an over-firing regression raises a false error on other people's code, and an over-separating one
// silently loses real duplicates — this fixture guards both directions.
//
// Every handler is distinct, so a finding can only come from a matcher rule.
import { Given, When } from "@cucumber/cucumber";

// Control: all three components agree.
Given("the alpha gate opens", () => alphaFirst());
Given("the alpha gate opens", () => alphaSecond());

// Same text, different KIND. Both comparison classes include `matcher_kind` (pairs.rs), and that
// is what separates these: removing only the `[regex-flags:...]` prefix from a regular expression's
// normalized form leaves this pair silent, while removing the kind from both keys collapses it.
Given("the beta valve turns", () => betaFirst());
Given(/the beta valve turns/, () => betaSecond());

// `i` is in the effective set, so it changes what the matcher accepts.
Given(/the gamma dial reads/, () => gammaFirst());
Given(/the gamma dial reads/i, () => gammaSecond());

// `s` is in the effective set: it changes whether `.` spans newlines.
Given(/the delta pump.runs/, () => deltaFirst());
Given(/the delta pump.runs/s, () => deltaSecond());

// Plainly different text.
Given("the epsilon lamp glows", () => epsilonFirst());
Given("the epsilon lamp shines", () => epsilonSecond());

// Effective flags, direction one: `g` is NOT in the effective set — it changes how a caller drives
// the regex, not the language accepted — so these two are an exact duplicate. The class used to key
// on the raw flag string and reported this as mere normalization equivalence, contradicting the
// documented contract.
Given(/the zeta rotor spins/, () => zetaFirst());
Given(/the zeta rotor spins/g, () => zetaSecond());

// Effective flags, direction two: the effective set is a set, so flag ORDER is not identity.
Given(/the eta brake holds/im, () => etaFirst());
Given(/the eta brake holds/mi, () => etaSecond());

// Registration keyword is not part of matcher identity: cucumber-js resolves a step by text, so the
// same matcher under `When` and `Given` claims the same step at runtime.
When("the theta clamp grips", () => thetaFirst());
Given("the theta clamp grips", () => thetaSecond());

// The other half of the cross-file property: this matcher's twin lives in `other-steps.ts`.
Given("the iota shaft drives", () => iotaInFirstFile());

// The declared semantic set is `['i','m','s','u','v']`. `i` and `s` are covered above and `v` in
// `regex-flag-identity`; these complete it. `m` changes what `^` and `$` anchor to.
Given(/^the nu piston strokes$/m, () => nuFirst());
Given(/^the nu piston strokes$/, () => nuSecond());

// `u` changes character-class and escape semantics.
Given(/the xi bearing rolls \w+/u, () => xiFirst());
Given(/the xi bearing rolls \w+/, () => xiSecond());

// And the cosmetic set beyond `g`: `d` only asks the engine for match indices, so it leaves the
// accepted language untouched and these two are an exact duplicate.
Given(/the omicron latch seats/d, () => omicronFirst());
Given(/the omicron latch seats/, () => omicronSecond());
