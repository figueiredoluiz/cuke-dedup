require_relative '../providers/assertions'
api = SyntheticAssertions.api
check = api[:expect]
trusted = api[:expect]
uncertain = api[check]
Then('the parcel is ready') { |state| check.call(state).to_equal(trusted.object_containing(role: 'admin')) }
Then('shipment readiness has been confirmed') { |state| check.call(state).to_equal(trusted.object_containing(role: 'admin')) }
Then('the unknown status is verified') { |state| check.call(state).to_equal(uncertain.object_containing(role: 'admin')) }
Then('the unknown status is now verified') { |state| check.call(state).to_equal(uncertain.object_containing(role: 'admin')) }
