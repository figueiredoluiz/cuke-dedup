# Property: a bracket delimiter nests by depth, so the inner pair does not close the literal.
# The second registration spells the first one's matcher with %r(...) around a nested (b) group,
# which makes a duplicate-matcher pair; closing at the first `)` would leave stray flag text and
# make the run incomplete. The third is the control: the same nested shape with another pattern.
Given(/\Apair (a(b)c)\z/) { first() }
Given(%r(\Apair (a(b)c)\z)) { second() }
Given(%r(\Apair (a(b)d)\z)) { control() }
