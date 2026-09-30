Feature: Step sources

  Background:
    Given the site is reachable

  Scenario: A plain scenario with a continuation
    Given the site is reachable
    And the report is archived

  Scenario Outline: Outline rows expand to concrete steps
    Given the <queue> queue is drained
    Then the audit trail is written

    Examples:
      | queue   |
      | primary |
      | standby |
