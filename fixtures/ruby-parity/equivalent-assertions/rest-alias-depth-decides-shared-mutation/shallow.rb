require_relative '../providers/assertions'
api = SyntheticAssertions.api
copy = api[:expect].dup
copy.not = replacement
Then('the pallet label is confirmed') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(tier: 'gold')) }
Then('the pallet label is now confirmed') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(tier: 'gold')) }
