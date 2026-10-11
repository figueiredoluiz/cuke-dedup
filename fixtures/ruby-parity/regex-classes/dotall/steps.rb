Given(/\Avalue a.b\z/m) { first() }
Then(%r{\Avalue a.b\z}m) { second() }
