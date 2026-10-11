require_relative 'providers/assertions'
Then('the parcel status is verified') { |state| SyntheticAssertions.expect(state).to_be('ready') }
Then('the parcel status is now verified') { |state| expected = 'ready'; SyntheticAssertions.expect(state).to_be(expected) }
