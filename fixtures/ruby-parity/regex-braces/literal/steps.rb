Given(/\Avalue %r{(.*)}\z/) { first() }
Then(%r{\Avalue %r{(.*)}\z}) { second() }
Then("value %r{text}") { control() }
