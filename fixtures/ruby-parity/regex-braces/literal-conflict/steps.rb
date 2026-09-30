Given(/\Avalue %r{(.*)}\z/) { first() }
Then(%r{\Avalue %r{(.*)}\z}) { second() }
Then("value %rtext") { control() }
