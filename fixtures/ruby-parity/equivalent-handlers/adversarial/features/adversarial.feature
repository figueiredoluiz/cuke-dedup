Feature: Adversarial parser behavior

  Scenario: Same identifier names in separate files
    Given the alpha operation runs
    When the beta operation runs

  Scenario Outline: Header-only examples retain their steps
    Given the pending alpha operation runs

    Examples:
      | unused |

  Scenario Outline: Placeholder values are substituted once
    Then the literal <a> and <b> value is retained

    Examples:
      | a   | b |
      | <b> | z |
