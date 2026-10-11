require_relative 'providers/assertions'
expect = ->(value) { value }
Then('the parcel status is now verified') { |state| expect.call(state).to_be('ready') }
