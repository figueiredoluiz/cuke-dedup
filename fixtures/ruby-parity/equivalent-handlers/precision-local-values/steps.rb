require_relative 'providers/assertions'
Then('the parcel status is verified') { |state| expected = 'ready'; SyntheticAssertions.expect(state).to_be(expected) }
Then('the parcel status is now verified') { |state| expected = 'idle'; SyntheticAssertions.expect(state).to_be(expected) }
