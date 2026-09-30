require 'world'
register = RegistrationProvider::GIVEN
register.call('package imports registration') { first_operation() }
register.call('package imports registration') { second_operation() }
