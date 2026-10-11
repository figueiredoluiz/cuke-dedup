require_relative 'providers/assertions'
Then('the parcel status is verified') { |state| inspect = ->(expect) { expect.call('unrelated') }; SyntheticAssertions.expect(state).to_be('ready') }
Then('the parcel status is now verified') { |state| inspect = ->(expect) { expect.call('unrelated') }; SyntheticAssertions.expect(state).to_be('idle') }
