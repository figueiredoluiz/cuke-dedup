# Purpose: `<b>` inside `the <a<b> value` is replaced by the Examples value; the stray `<a` stays verbatim.
Feature: Stray angle bracket
  Scenario Outline: nested angle bracket
    Given the <a<b> value
    Examples:
      | b |
      | 7 |
