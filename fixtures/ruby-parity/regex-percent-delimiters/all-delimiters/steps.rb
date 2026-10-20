# Property: every %r delimiter registers like the slash literal. The five %r lines after the first
# spell its matcher with another delimiter, so recognizing them is what produces the six-way
# duplicate-matcher cluster and the ambiguity with the feature step; an unrecognized delimiter
# would make the run incomplete. The `other` line is the control: the same delimiter with another
# pattern joins no cluster.
Given(/\Aitem 1\z/) { first() }
Given(%r!\Aitem 1\z!) { second() }
Given(%r[\Aitem 1\z]) { third() }
Given(%r<\Aitem 1\z>) { fourth() }
Given(%r|\Aitem 1\z|) { fifth() }
Given(%r{\Aitem 1\z}) { sixth() }
Given(%r!\Aother 1\z!) { control() }
