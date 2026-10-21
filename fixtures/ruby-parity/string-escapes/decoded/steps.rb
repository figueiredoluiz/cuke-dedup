# Pins: double-quoted matchers decode \s, \x and octal escapes (including the bytes of a UTF-8
# character) to the value Ruby builds, so each escaped matcher duplicates its plain twin. The last
# pair differs by one escaped character (\x42 is B, not A) and must stay distinct: its escaped
# matcher is the only unused definition. An implementation that left escapes undecoded would
# report the run incomplete instead of finding the first three pairs.
Given("tab\sseparated") { first() }
Given('tab separated') { second() }
Given("item \x41 \101") { third() }
Given('item A A') { fourth() }
Given("caf\xC3\xA9") { fifth() }
Given('café') { sixth() }
Given("mix \x42") { seventh() }
Given('mix A') { eighth() }
