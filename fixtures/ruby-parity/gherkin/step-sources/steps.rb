# Reachable only from a Background step.
Given('the site is reachable') { reachable() }

# Reachable only by expanding a Scenario Outline row from Examples.
Given('the {word} queue is drained') { drained() }

# Reachable only through a Then continuation inside the outline.
Then('the audit trail is written') { audited() }

# Reachable only through an And continuation in a plain scenario.
Given('the report is archived') { archived() }

# Matched by nothing: the positive control.
Given('the vault is sealed') { sealed() }
