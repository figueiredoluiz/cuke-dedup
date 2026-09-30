require_relative 'assertions'
module FormFixture
  THEN = method(:Then)
  EXPECT = SyntheticAssertions.method(:expect)
end
