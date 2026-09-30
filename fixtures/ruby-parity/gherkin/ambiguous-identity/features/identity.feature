Feature: Ambiguity identity

  Scenario: Two matchers accept one step
    Given the alpha gauge reads high

  Scenario: Three matchers accept one step
    Given the beta dial holds firm

  Scenario Outline: Rows sharing a match set coalesce
    Given the gamma pump moves <count>

    Examples:
      | count |
      | 1     |
      | 2     |
      | 3     |

  Scenario Outline: Rows with different match sets stay distinct
    Given the delta valve turns <way>

    Examples:
      | way   |
      | left  |
      | right |

  Scenario: First authored location
    Given the epsilon lamp glows dim

  Scenario: Second authored location
    Given the epsilon lamp glows dim

  Scenario: An undeclared parameter type proves nothing
    Given the zeta rotor spins fast

  Scenario: A single match is not ambiguous
    Given the eta brake holds tight
