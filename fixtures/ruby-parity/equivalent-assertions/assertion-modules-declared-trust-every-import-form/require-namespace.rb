require './providers/assertions'
api = SyntheticAssertions.api
Given('the delta sensor logs a first sample') { || api.fetch(:expect).call(sensor()).to_be(1) }
Given('the delta sensor logs a second sample') { || api.fetch(:expect).call(sensor()).to_be(2) }
