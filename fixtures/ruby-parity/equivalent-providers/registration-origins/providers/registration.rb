module RegistrationProvider
  GIVEN = method(:Given)
  WHEN = method(:When)
  THEN = method(:Then)
  def self.register(text, &handler)
    GIVEN.call(text, &handler)
  end
end

module OrdinaryNamespace
  def self.Given(text, &handler)
    [text, handler]
  end
end
