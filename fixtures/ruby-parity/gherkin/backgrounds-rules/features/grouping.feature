Feature: Grouping
  Background:
    Given the shared setup is ready

  Scenario: First
    When the first action runs

  Scenario: Second
    Then the second result appears

  Rule: Grouped behavior
    Background:
      Given the rule setup is ready

    Scenario: Third
      When the third action runs
