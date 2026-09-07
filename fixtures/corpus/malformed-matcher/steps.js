Given(/value (?=ahead)/, () => inspectAhead());
Given('broken \xZZ', () => brokenMatcher());
Given('the valid operation runs', () => runValidOperation());
