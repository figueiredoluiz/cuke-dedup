require_relative "providers/registration"
require_relative "providers/handlers"
register = RegistrationProvider::GIVEN
namespace = RegistrationProvider
renamed = register
given_alias = register
namespace_alias = namespace
register.call('the alpha hatch opens {word}') { origin_alpha_1() }
register.call('the alpha hatch opens wide') { origin_alpha_2() }
RegistrationProvider.register('the beta valve turns {word}') { origin_beta_1() }
RegistrationProvider.register('the beta valve turns left') { origin_beta_2() }
RegistrationProvider.register('the gamma dial reads {word}') { origin_gamma_1() }
RegistrationProvider.register('the gamma dial reads high') { origin_gamma_2() }
renamed.call('the delta pump runs {word}') { origin_delta_1() }
renamed.call('the delta pump runs fast') { origin_delta_2() }
Given('the epsilon lamp glows {word}') { origin_epsilon_1() }
Given('the epsilon lamp glows dim') { origin_epsilon_2() }
namespace_alias.register('the zeta rotor spins {word}') { origin_zeta_1() }
namespace_alias.register('the zeta rotor spins clockwise') { origin_zeta_2() }
given_alias.call('the eta brake holds {word}') { origin_eta_1() }
given_alias.call('the eta brake holds firm') { origin_eta_2() }
Given('the theta clamp grips {word}') { origin_theta_1() }
Given('the theta clamp grips tight') { origin_theta_2() }
register.call('the kappa gear meshes {word}', &-> { origin_kappa_1() })
register.call('the kappa gear meshes cleanly', &-> { origin_kappa_2() })
register.call('the lambda seal closes {word}', &OriginHandlers.new.method(:lambda_1))
register.call('the lambda seal closes flush', &OriginHandlers.new.method(:lambda_2))
register.call('the mu spring loads {word}', &OriginHandlers.new.method(:mu_1))
register.call('the mu spring loads slowly', &OriginHandlers.new.method(:mu_2))
RegistrationProvider.register('the nu piston strokes {word}') { origin_nu_1() }
RegistrationProvider.register('the nu piston strokes long') { origin_nu_2() }
RegistrationProvider.register('the xi bearing rolls {word}') { origin_xi_1() }
RegistrationProvider.register('the xi bearing rolls freely') { origin_xi_2() }
renamed.call('the rho cable tightens {word}') { origin_rho_1() }
renamed.call('the rho cable tightens fully') { origin_rho_2() }
register.call('the tau chain feeds {word}') { origin_tau_1() }
register.call('the tau chain feeds evenly') { origin_tau_2() }
untrusted = ->(text, &handler) { [text, handler] }
untrusted.call('the omicron latch seats {word}') { false_origin_one() }
untrusted.call('the omicron latch seats fully') { false_origin_two() }
shadowed = ->(text, &handler) { text }
shadowed.call('the pi rod extends {word}') { shadow_one() }
shadowed.call('the pi rod extends far') { shadow_two() }
given = ->(text, &handler) { handler }
given.call('the iota shaft drives {word}') { lower_one() }
given.call('the iota shaft drives hard') { lower_two() }
erased_analogue = ->(text, &handler) { nil }
erased_analogue.call('the upsilon belt slips {word}') { erased_one() }
erased_analogue.call('the upsilon belt slips loose') { erased_two() }
