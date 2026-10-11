Given(/\Avalue []a{,3}]\z/) { first() }
Then(%r{\Avalue []a{,3}]\z}) { second() }
Then("value 0") { control() }
