# Four definitions remain three pair findings.
Given('the alpha gate opens') { alpha1() }
Given('the alpha gate opens') { alpha2() }
Given('the alpha gate opens') { alpha3() }
Given('the alpha gate opens') { alpha4() }

# Five definitions cross the cluster boundary and remain one complete cluster.
Given('the beta valve turns') { beta1() }
Given('the beta valve turns') { beta2() }
Given('the beta valve turns') { beta3() }
Given('the beta valve turns') { beta4() }
Given('the beta valve turns') { beta5() }

# Three definitions confirm n-1 pair findings in the lower range.
Given('the delta pump runs') { delta1() }
Given('the delta pump runs') { delta2() }
Given('the delta pump runs') { delta3() }

# A second five-member group confirms clustering also applies to identical regex matchers.
Given(/^the gamma dial reads$/) { gamma1() }
Given(/^the gamma dial reads$/) { gamma2() }
Given(/^the gamma dial reads$/) { gamma3() }
Given(/^the gamma dial reads$/) { gamma4() }
Given(/^the gamma dial reads$/) { gamma5() }
