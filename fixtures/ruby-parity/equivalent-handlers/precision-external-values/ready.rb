require_relative 'providers/assertions'
expected = 'ready'
Then('the parcel status is verified') { |state| SyntheticAssertions.expect(state).to_be(expected) }
