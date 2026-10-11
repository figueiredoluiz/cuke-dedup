handler = -> { perform_alpha_operation() }
not_implemented = -> { raise 'pending' }
Given('the alpha operation runs', &handler)
Given('the pending alpha operation runs', &not_implemented)
