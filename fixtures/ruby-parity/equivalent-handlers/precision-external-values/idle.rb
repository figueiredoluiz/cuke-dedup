require_relative 'providers/assertions'
expected = 'idle'
Then('the parcel status is now verified') { |state| SyntheticAssertions.expect(state).to_be(expected) }
