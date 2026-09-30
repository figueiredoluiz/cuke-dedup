Feature: Escaped table paths
  Scenario Outline: Preserve path cell escapes
    Given path <value>

    Examples: Path spellings
      | value        |
      | C:\work\path |
      | left\|right  |
      | C:\\tools    |
