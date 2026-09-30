# Two matchers accept one step.
Given('the alpha gauge reads {word}') { a1() }
Given(/^the alpha gauge reads high$/) { a2() }

# Three matchers accept one step.
Given('the beta dial holds {word}') { b1() }
Given(/^the beta dial holds .+$/) { b2() }
Given('the beta dial holds firm') { b3() }

# Outline rows sharing one match set coalesce into one finding.
Given('the gamma pump moves {int}') { c1() }
Given(/^the gamma pump moves \d+$/) { c2() }

# Different row match sets remain distinct at one template location.
Given('the delta valve turns {word}') { d1() }
Given(/^the delta valve turns left$/) { d2() }
Given(/^the delta valve turns right$/) { d3() }

# The same authored text at two locations remains two locations.
Given('the epsilon lamp glows {word}') { e1() }
Given(/^the epsilon lamp glows dim$/) { e2() }

# An undeclared custom parameter type cannot prove ambiguity.
Given('the zeta rotor spins {speed}') { f1() }
Given('the zeta rotor spins fast') { f2() }

# One matching definition is not ambiguous.
Given('the eta brake holds tight') { g1() }
