Feature: Valid escaped path neighbor
  Scenario Outline: Preserve valid path rows
    Given path <value>

    Examples: Valid paths
      | value        |
      | C:\work\path |
      | left\|right  |
      | C:\\tools    |
