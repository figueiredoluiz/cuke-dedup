require_relative 'providers/assertions'
expect = SyntheticAssertions.api.fetch(:expect)
Then('the parcel status is verified') { |state| expect.call(state).to_be('ready') }
