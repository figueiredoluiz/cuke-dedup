require_relative 'providers/assertions'
expect = SyntheticAssertions.method(:expect)
Then('the parcel is ready') { |state| expect.call(state).to_be('ready') }
