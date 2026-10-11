require_relative 'providers/assertions'
ALPHA_LIMIT = 1
ALPHA_BOUND = 1
Given('the alpha meter reaches its limit') { SyntheticAssertions.expect(meter()).to_be(ALPHA_LIMIT) }
Given('the alpha meter reaches its bound') { SyntheticAssertions.expect(meter()).to_be(ALPHA_BOUND) }
BETA_FIRST = 1
BETA_SECOND = 2
Given('the beta dial holds its first stop') { SyntheticAssertions.expect(dial()).to_be(BETA_FIRST) }
Given('the beta dial holds its second stop') { SyntheticAssertions.expect(dial()).to_be(BETA_SECOND) }
gamma_first = 1
gamma_second = 1
Given('the gamma probe tracks a mutable first point') { SyntheticAssertions.expect(probe()).to_be(gamma_first) }
Given('the gamma probe tracks a mutable second point') { SyntheticAssertions.expect(probe()).to_be(gamma_second) }
# Reassignable captured locals do not establish immutable values for deferred handlers.
TOP_ONE = 1
TOP_TWO = 1
Given('the top slot is pressed first') { click_button(TOP_ONE) }
Given('the top slot is pressed second') { click_button(TOP_TWO) }
ALPHA_LABEL = 'ready'.freeze
ALPHA_TAG = 'ready'.freeze
Given('the alpha panel shows its label') { SyntheticAssertions.expect(panel()).to_be(ALPHA_LABEL) }
Given('the alpha panel shows its tag') { SyntheticAssertions.expect(panel()).to_be(ALPHA_TAG) }
shared_stop = 1
Given('the omega brake holds a local stop') { |; shared_stop| shared_stop = 2; SyntheticAssertions.expect(brake()).to_be(shared_stop) }
Given('the omega brake holds a module stop') { SyntheticAssertions.expect(brake()).to_be(shared_stop) }
Given('the delta valve settles at its early mark') { SyntheticAssertions.expect(valve()).to_be(DELTA_EARLY) }
Given('the delta valve settles at its late mark') { SyntheticAssertions.expect(valve()).to_be(DELTA_LATE) }
DELTA_EARLY = 1
DELTA_LATE = 1
Given('the epsilon lever locks at its low notch') { SyntheticAssertions.expect(lever()).to_be(EPSILON_LOW) }
Given('the epsilon lever locks at its high notch') { SyntheticAssertions.expect(lever()).to_be(EPSILON_HIGH) }
EPSILON_LOW = 1
EPSILON_HIGH = 2
