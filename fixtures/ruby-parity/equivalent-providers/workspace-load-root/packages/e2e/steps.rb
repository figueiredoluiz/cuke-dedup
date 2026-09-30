require 'support'
register = RegistrationProvider::GIVEN
register.call('workspace package registration') { first_operation() }
register.call('workspace package registration') { second_operation() }
