require_relative '../providers/assertions'
api = SyntheticAssertions.api
negated = api.fetch(:expect).not
Then('the crate label is confirmed') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(role: 'editor')) }
Then('the crate label is now confirmed') { |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(role: 'editor')) }
