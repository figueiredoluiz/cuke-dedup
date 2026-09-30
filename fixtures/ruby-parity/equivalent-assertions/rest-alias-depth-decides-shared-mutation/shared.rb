require_relative '../providers/assertions'
api = SyntheticAssertions.api
copy = api[:expect].dup
copy.not.object_containing = replacement
Then('the crate seal is verified') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(tier: 'gold')) }
Then('the crate seal is now verified') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(tier: 'gold')) }
