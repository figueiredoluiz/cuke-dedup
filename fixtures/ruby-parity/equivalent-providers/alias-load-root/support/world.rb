require_relative 'registration'
MAIN_REGISTRATION_GIVEN = method(:Given)
MAIN_REGISTRATION_WHEN = method(:When)
MAIN_REGISTRATION_THEN = method(:Then)
module RegistrationProvider
  GIVEN = ::MAIN_REGISTRATION_GIVEN
  WHEN = ::MAIN_REGISTRATION_WHEN
  THEN = ::MAIN_REGISTRATION_THEN
  def self.register(text, &handler)
    GIVEN.call(text, &handler)
  end
end
