require 'world'
register = RegistrationProvider::GIVEN
register.call('aliased registration') { first_operation() }
register.call('aliased registration') { second_operation() }
