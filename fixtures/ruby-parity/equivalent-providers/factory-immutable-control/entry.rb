require_relative 'providers/assertions'
api = SyntheticAssertions.api
check = api[:expect]
Then('the parcel status is verified') do |state|
  api[:expect].call(state).to_equal(api[:expect].object_containing(role: 'admin'))
end
Then('the parcel status is now verified') do |state|
  api[:expect].call(state).to_equal(api[:expect].object_containing(role: 'admin'))
end
