require_relative '../support/index'
register = RegistrationProvider::GIVEN
register.call('cross package registration') { first_operation() }
register.call('cross package registration') { second_operation() }
