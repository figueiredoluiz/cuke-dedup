require_relative '../providers/assertions'
check = SyntheticAssertions.method(:expect)
Given('the alpha meter shows a first value') { || check.call(meter()).to_be(1) }
Given('the alpha meter shows a second value') { || check.call(meter()).to_be(2) }
