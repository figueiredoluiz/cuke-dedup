Feature: Reject unterminated example row
  Scenario Outline: Ignore the malformed feature
    Given path <value>

    Examples: Unterminated paths
      | value |
      | single
