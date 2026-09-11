Feature: Authenticated workspace
  Scenario: Open a protected page
    Given an authenticated session exists
    Then the protected dashboard is ready
