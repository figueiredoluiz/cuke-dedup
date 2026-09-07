# Checkout: overview

## Scenario: Markdown lists

+ Given a documented step
1. When an ordered step runs
* **Then** a bold step works

### Notes

* first prose note

```text
This is documentation, not a step doc string.
```

## Scenario Outline: GFM table

* Given value <value>

### Examples:

  | value |
  | - |
  | one |
