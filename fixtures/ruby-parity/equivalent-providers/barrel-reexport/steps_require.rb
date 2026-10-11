require_relative 'barrel'
register_via_require = RegistrationBarrel::REGISTER
register_via_require.call('cjs barrel require registration') { third_operation() }
register_via_require.call('cjs barrel require registration') { fourth_operation() }
