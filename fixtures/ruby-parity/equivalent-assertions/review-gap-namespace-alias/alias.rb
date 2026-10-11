require_relative '../providers/assertions'
api = SyntheticAssertions.api
other = api
other[:expect] = replacement
Then('the parcel status is verified') { |state| api[:expect].call(state).to_be('ready') }
