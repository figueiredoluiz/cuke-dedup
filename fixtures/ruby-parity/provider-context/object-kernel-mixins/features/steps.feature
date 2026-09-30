Feature: Object and Kernel method scope
  Scenario: Explicit DSL extension retains precedence
    Given shipment is ready
    Then shipment is now ready
