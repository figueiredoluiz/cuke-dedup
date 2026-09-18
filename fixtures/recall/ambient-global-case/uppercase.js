// Purpose: positive control for ambient registration globals. Cucumber injects `Given` without an
// import, and the duplicate must still be found. If this stops firing, the ambient path is broken
// and the known miss below would be meaningless.
Given("the alpha gauge is settled", () => work());
Given("the alpha gauge is settled", () => work());
