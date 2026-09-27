Feature: Parcel status
  Scenario: Ready parcel
    Given the parcel is ready
  Scenario: Ready parcel again
    Given the parcel is ready
  Scenario: Ready parcel, different wording
    Given shipment is ready
  Scenario: Idle parcel
    Given the parcel is idle
  Scenario: Plug-in registration is separate
    Given plugin parcel is ready
