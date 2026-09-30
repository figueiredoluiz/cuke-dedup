require_relative '../providers/assertions'
api = SyntheticAssertions.api
negated = api.fetch(:expect).not
negated.object_containing = replacement
Then('the parcel status is verified') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(role: 'admin')) }
Then('the parcel status is now verified') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(role: 'admin')) }
