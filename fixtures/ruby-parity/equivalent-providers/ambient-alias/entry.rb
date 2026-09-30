Given('the alpha gauge is settled') { calibrate_alpha() }
Given('the alpha gauge is settled') { inspect_alpha() }
given = method(:Given)
given.call('the beta dial is settled') { calibrate_beta() }
given.call('the beta dial is settled') { inspect_beta() }
require_relative 'lowercase_negative'
