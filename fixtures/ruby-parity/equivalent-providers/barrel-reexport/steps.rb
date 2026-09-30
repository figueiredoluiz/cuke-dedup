require_relative 'barrel'
register_step = RegistrationBarrel::REGISTER
register_step.call('cjs barrel registration') { first_operation() }
register_step.call('cjs barrel registration') { second_operation() }
