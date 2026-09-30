require_relative 'providers/assertions'
helper = -> { delta_one = 1; delta_two = 1; deep_one = 1; deep_two = 1; [delta_one, delta_two, deep_one, deep_two] }
Given('the delta sensor samples a nested first slot') { SyntheticAssertions.expect(sensor()).to_be(delta_one) }
Given('the delta sensor samples a nested second slot') { SyntheticAssertions.expect(sensor()).to_be(delta_two) }
Given('the nested slot is pressed first') { click_button(deep_one) }
Given('the nested slot is pressed second') { click_button(deep_two) }
captured_limit = 1
build_first = lambda do |; captured_limit|
  captured_limit = 7
  Given('the capstan winds to a first limit') { SyntheticAssertions.expect(capstan()).to_be(captured_limit) }
end
build_second = lambda do |; captured_limit|
  captured_limit = 8
  Given('the capstan winds to a second limit') { SyntheticAssertions.expect(capstan()).to_be(captured_limit) }
end
build_first.call()
build_second.call()
captured_span = 1
span_first = lambda do |captured_span|
  Given('the derrick swings to a first span') { SyntheticAssertions.expect(derrick()).to_be(captured_span) }
end
span_second = lambda do |captured_span|
  Given('the derrick swings to a second span') { SyntheticAssertions.expect(derrick()).to_be(captured_span) }
end
span_first.call(7)
span_second.call(8)
