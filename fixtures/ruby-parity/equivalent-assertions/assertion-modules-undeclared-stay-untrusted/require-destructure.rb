require './providers/assertions'
check, = SyntheticAssertions.api.values_at(:expect)
Given('the epsilon valve reports a first state') { || check.call(valve()).to_be(1) }
Given('the epsilon valve reports a second state') { || check.call(valve()).to_be(2) }
