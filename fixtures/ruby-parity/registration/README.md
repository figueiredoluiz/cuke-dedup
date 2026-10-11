# Registration outcomes

Each child directory is a separate analysis case. `identical` is the positive exact-handler control; `conflicting` proves matcher equality alone does not yield duplicate-handler; `uncertain` and `scope` require incomplete analysis and zero trusted definitions. Do not combine these files because suite-wide DSL invalidation would contaminate the controls.
