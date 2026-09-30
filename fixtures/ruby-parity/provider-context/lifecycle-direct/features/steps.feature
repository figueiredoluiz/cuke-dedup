Feature: Provider lifecycle controls
  Scenario: Ready status
    Given shipment is ready
    Then shipment is now ready

  Scenario: Rejected status
    Given shipment is rejected
    Then shipment is now rejected
