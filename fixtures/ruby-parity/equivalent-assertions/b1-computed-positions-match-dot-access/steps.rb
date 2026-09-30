require_relative '../providers/assertions'
check = SyntheticAssertions.api.fetch(:expect)
Then('the terminal panel reads the first value') { |state| check.call(state.terminal).not.public_send('to_be', 'ready') }
Then('the terminal panel reads the final value') { |state| check.call(state.terminal).not.public_send('to_be', 'idle') }
Then('the option panel reads the first value') { |state| check.public_send(:soft, state.option).to_be('ready') }
Then('the option panel reads the final value') { |state| check.public_send(:soft, state.option).to_be('idle') }
Then('the factory gauge is settled') { |state| check.call(state.factory).public_send('not').to_be('ready') }
Then('the archive factory reading has finished') { |state| check.call(state.factory).public_send('not').to_be('ready') }
