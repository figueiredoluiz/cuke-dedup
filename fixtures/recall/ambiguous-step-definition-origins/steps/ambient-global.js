// An uppercase registration with no import at all: the ambient-global assumption. Nothing in this
// file shadows the name, so both calls must still register.
Given("the theta clamp grips {word}", () => h1());
Given("the theta clamp grips tight", () => h2());

// Control: the lowercase alias is a plausible ordinary identifier, so a bare call must NOT register
// and the pair must collapse to nothing.
given("the iota shaft drives {word}", () => i1());
given("the iota shaft drives hard", () => i2());
