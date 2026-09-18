// Purpose: records the lowercase ambient global as a known miss. Cucumber also injects lowercase
// aliases, so this pair is a real duplicate that the analyzer does not yet see: the file yields no
// definitions at all. Destructuring `given` from the package IS recognised, so the gap is specific
// to the ambient spelling.
given("the beta dial is settled", () => work());
given("the beta dial is settled", () => work());
