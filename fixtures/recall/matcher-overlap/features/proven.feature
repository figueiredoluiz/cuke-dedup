Feature: Proven ambiguity

  # Purpose: prove exactly one ambiguity, with the minimal input that does so, so that
  # `ambiguous-step` owns the `eta` pair and `overlapping-matcher` stays silent for it. Deliberately
  # a single scenario with a single step: this is the negative control for overlap, and the simple
  # single-scenario extraction path is part of what it pins. Gherkin construct breadth belongs in a
  # case whose purpose is construct breadth, not here.
  Scenario: A concrete step matches both definitions
    Given the eta hatch opens wide
