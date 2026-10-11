require_relative 'providers/assertions'
Then('the parcel is ready') { |state| expected = 'ready'; SyntheticAssertions.expect(state).to_be(expected) }
Then('shipment readiness has been confirmed') { |state| desired = 'ready'; SyntheticAssertions.expect(state).to_be(desired) }
