require_relative 'assertions'
MAIN_REGISTRATION_THEN = method(:Then)
module FormFixture
  THEN = ::MAIN_REGISTRATION_THEN
  EXPECT = SyntheticAssertions.method(:expect)
end
