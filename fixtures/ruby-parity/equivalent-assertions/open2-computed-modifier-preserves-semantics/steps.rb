require_relative '../providers/assertions'
check = SyntheticAssertions.api.fetch(:expect)
Then('the computed panel shows the first state') { |state| check.call(state.status).public_send('not').to_be('ready') }
Then('the computed panel shows the final state') { |state| check.call(state.status).public_send('not').to_be('idle') }
Then('the inventory badge is settled') { |state| check.call(state.badge).public_send('not').to_be('ready') }
Then('the archive marker has finished loading') { |state| check.call(state.badge).public_send('not').to_be('ready') }
Then('the gauge reading is checked') { |state| check.call(state.gauge).public_send('not').to_be('ready') }
Then('the gauge reading is now checked') { |state| check.call(state.gauge).to_be('ready') }
