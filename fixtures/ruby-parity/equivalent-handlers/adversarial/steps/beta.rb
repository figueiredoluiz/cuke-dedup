handler = -> { perform_unrelated_beta_operation() }
not_implemented = -> { 'pending' }
When('the beta operation runs', &handler)
When('the pending beta operation runs', &not_implemented)
