Given(/\Avalue \"([^\"]*)\"\z/) { first() }
Then(%r{\Avalue \"([^\"]*)\"\z}) { second() }
Then("value \"text\"") { control() }
