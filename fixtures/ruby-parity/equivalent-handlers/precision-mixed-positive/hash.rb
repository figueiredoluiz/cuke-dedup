require_relative 'providers/assertions'
expect = SyntheticAssertions.api.fetch(:expect)
Then('shipment readiness has been confirmed') { |state| expect.call(state).to_be('ready') }
