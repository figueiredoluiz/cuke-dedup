require_relative 'providers/assertions'
Given('the alpha meter settles evenly') do
  check = -> { SyntheticAssertions.expect(meter()).to_be(1) }; check.call
end
Given('the alpha meter settles smoothly') do
  check = -> { SyntheticAssertions.expect(meter()).to_be(1) }; check.call
end
Given('the beta dial resolves upward') do
  check = -> { SyntheticAssertions.expect(dial()).to_be(1) }; check.call
end
Given('the beta dial resolves downward') do
  check = -> { SyntheticAssertions.expect(dial()).to_be(2) }; check.call
end
Given('the gamma probe reads a shadowed first value') do
  check = -> { SyntheticAssertions.expect(probe()).to_be(9) }
  1.times do |iteration; check|
    check = -> { SyntheticAssertions.expect(probe()).to_be(1) }
    check.call
  end
end
Given('the gamma probe reads a shadowed second value') do
  check = -> { SyntheticAssertions.expect(probe()).to_be(9) }
  1.times do |iteration; check|
    check = -> { SyntheticAssertions.expect(probe()).to_be(2) }
    check.call
  end
end
Given('the delta sensor logs a replaced first sample') do
  check = -> { SyntheticAssertions.expect(sensor()).to_be(9) }
  check = -> { SyntheticAssertions.expect(sensor()).to_be(1) }
  check.call
end
Given('the delta sensor logs a replaced second sample') do
  check = -> { SyntheticAssertions.expect(sensor()).to_be(9) }
  check = -> { SyntheticAssertions.expect(sensor()).to_be(2) }
  check.call
end
Given('the relay trips on a first path') do
  act = -> { SyntheticAssertions.expect(relay()).to_be(1) }
  act = -> { SyntheticAssertions.expect(relay()).to_be(9) }
  act.call
end
Given('the relay trips on a second path') do
  act = -> { SyntheticAssertions.expect(relay()).to_be(2) }
  act = -> { SyntheticAssertions.expect(relay()).to_be(9) }
  act.call
end
Given('the crane idles at a first angle') do
  act = -> { SyntheticAssertions.expect(crane()).to_be(1) }
end
Given('the crane idles at a second angle') do
  act = -> { SyntheticAssertions.expect(crane()).to_be(2) }
end
Given('the pulley turns to a first notch') do
  act = ->(value) { SyntheticAssertions.expect(pulley()).to_be(value) }
  act.call(1)
end
Given('the pulley turns to a second notch') do
  act = ->(value) { SyntheticAssertions.expect(pulley()).to_be(value) }
  act.call(2)
end
Given('the spindle cycles a first turn') do
  act = -> { SyntheticAssertions.expect(spindle()).to_be(1); act.call }
  act.call
end
Given('the spindle cycles a second turn') do
  act = -> { SyntheticAssertions.expect(spindle()).to_be(2); act.call }
  act.call
end
Given('the gantry shifts a first span') do
  outer = -> { act = -> { SyntheticAssertions.expect(gantry()).to_be(1) } }
  act.call
end
Given('the gantry shifts a second span') do
  outer = -> { act = -> { SyntheticAssertions.expect(gantry()).to_be(2) } }
  act.call
end
Given('the ratchet holds a first tooth') do
  act = -> { SyntheticAssertions.expect(ratchet()).to_be(1) }
  1.times do |iteration; act|
    act = -> { ratchet_shadow() }
    act.call
  end
end
Given('the ratchet holds a second tooth') do
  act = -> { SyntheticAssertions.expect(ratchet()).to_be(2) }
  1.times do |iteration; act|
    act = -> { ratchet_shadow() }
    act.call
  end
end
Given('the hoist lifts along a first track') do
  act = -> { SyntheticAssertions.expect(hoist()).to_be(1) }
  (act).call
end
Given('the hoist lifts along a second track') do
  act = -> { SyntheticAssertions.expect(hoist()).to_be(2) }
  (act).call
end
Given('the winch spools past a first mark') do
  act = -> { SyntheticAssertions.expect(winch()).to_be(1) }
  1.times do |iteration; act|
    act = -> { winch_shadow() }
    act.call
  end
end
Given('the winch spools past a second mark') do
  act = -> { SyntheticAssertions.expect(winch()).to_be(2) }
  1.times do |iteration; act|
    act = -> { winch_shadow() }
    act.call
  end
end
