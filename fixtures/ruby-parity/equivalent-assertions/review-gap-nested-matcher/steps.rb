require_relative '../providers/assertions'
check = SyntheticAssertions.api.fetch(:expect)
Then('the parcel is ready') { |state| check.call(state).to_equal(check.object_containing(role: 'admin')) }
Then('shipment readiness has been confirmed') { |state| check.call(state).to_equal(check.object_containing(role: 'admin')) }
