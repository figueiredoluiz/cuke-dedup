require_relative '../providers/assertions'
api = SyntheticAssertions.api
Then('the parcel status is now verified') { |state| api[:expect].call(state).to_be('ready') }
