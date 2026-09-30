require_relative 'providers/unrelated'
expect = UnrelatedAssertions.api.fetch(:expect)
Then('the parcel status is now verified') { |state| expect.call(state).to_be('ready') }
