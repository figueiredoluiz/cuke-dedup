Feature: Reject ragged example table
  Scenario Outline: Ignore the malformed feature
    Given path <value>

    Examples: Ragged paths
      | value      |
      | single | extra |
