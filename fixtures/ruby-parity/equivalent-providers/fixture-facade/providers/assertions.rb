# Original synthetic provider. Trust requires an explicit assertionModules entry.
module SyntheticAssertions
  class Matcher
    def initialize(&predicate)
      @predicate = predicate
    end
    def matches?(actual)
      @predicate.call(actual)
    end
  end

  class Expectation
    def initialize(actual, negated = false)
      @actual, @negated = actual, negated
    end
    def not
      self.class.new(@actual, !@negated)
    end
    def to
      self
    end
    def verify(result)
      raise 'synthetic assertion failed' if result == @negated
      true
    end
    def to_be(expected)
      verify(@actual == expected)
    end
    alias equal_to to_be
    def to_equal(expected)
      verify(expected.is_a?(Matcher) ? expected.matches?(@actual) : @actual == expected)
    end
    def to_have_class(pattern)
      verify(pattern.match?(@actual))
    end
    def to_have_value(expected)
      verify(@actual.value == expected)
    end
    def to_be_visible
      verify(@actual.visible?)
    end
  end

  class Factory
    attr_accessor :not
    def initialize(exported_expect, negated = false)
      @exported_expect = exported_expect
      @builder = lambda { |entries| Matcher.new { |actual| entries.all? { |key, value| actual[key] == value } != negated } }
      @not = Factory.new(exported_expect, true) unless negated
    end
    def call(actual)
      @exported_expect.call(actual)
    end
    alias soft call
    def object_containing(entries)
      @builder.call(entries)
    end
    def object_containing=(replacement)
      @builder = replacement
    end
  end

  def self.api
    { expect: Factory.new(method(:expect)) }
  end
  def self.expect(actual)
    Expectation.new(actual)
  end
end
