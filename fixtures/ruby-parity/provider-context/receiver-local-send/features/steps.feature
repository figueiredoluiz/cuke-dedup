Feature: Receiver-local dispatch
  Scenario: Global step definitions stay available
    Given shipment is ready
    Then shipment is now ready
