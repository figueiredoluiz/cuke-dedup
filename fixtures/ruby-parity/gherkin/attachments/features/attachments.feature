@checkout
Feature: Attached data

  @basket
  Scenario: Basket contents
    Given the basket is ready
      | item   | count |
      | apples | 2     |
    When the order is submitted
    Then the receipt contains
      """
      a displayed value
      """
