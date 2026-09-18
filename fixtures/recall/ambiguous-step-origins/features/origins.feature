Feature: Step origins

  Background:
    Given the beta valve turns left

  Scenario: A plain scenario step
    Given the alpha hatch opens wide

  Scenario: A continuation step
    Given an unrelated preparation
    And the delta pump runs fast

  Scenario: An asterisk step
    * the epsilon lamp glows dim

  Scenario: A but continuation step
    Given an unrelated preparation
    But the mu spring loads slowly

  Scenario: Keywords do not gate matching
    When the iota shaft drives hard
    Then the kappa gear meshes cleanly
    And the lambda seal closes flush

  Scenario: A unique match is not an origin of ambiguity
    Given the omicron latch seats fully

  Scenario Outline: An outline row expanded from examples
    Given the gamma dial reads <level>

    Examples:
      | level |
      | high  |

  Rule: A rule block groups scenarios
    Background:
      Given the theta clamp grips tight

    Scenario: A step inside a rule block
      Given the zeta rotor spins clockwise

  Scenario Template: A template with scenarios
    Given the eta brake holds <grip>

    Scenarios:
      | grip |
      | firm |
