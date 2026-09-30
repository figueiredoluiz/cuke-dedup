require_relative '../providers/assertions'
check = SyntheticAssertions.api.fetch(:expect)
negated = check.public_send('not')
negated.object_containing = replacement
Then('the parcel status is verified') { |state| check.call(state).to_equal(check.not.object_containing(role: 'admin')) }
Then('the parcel status is now verified') { |state| check.call(state).to_equal(check.not.object_containing(role: 'admin')) }
