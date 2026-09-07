defineParameterType({ name: "duration", regexp: /\d+ seconds/ });

Given("I wait {duration}", () => Promise.resolve());
Given("I wait for the page", () => page.waitForLoadState());
Given("a documented step", () => openDocumentation());
When("an ordered step runs", () => runOrderedStep());
Then("a bold step works", () => verifyBoldStep());
Given("value {word}", (value) => selectValue(value));
