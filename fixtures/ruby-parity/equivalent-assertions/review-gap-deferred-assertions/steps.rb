require_relative '../providers/assertions'
check = SyntheticAssertions.api.fetch(:expect)
Then('the archive badge is visible') { |state| register(callback: -> { check.call(state.archive).to_be('ready') }) }
Then('the archive badge is now visible') { |state| other_wrapper([proc { check.call(state.archive).to_be('ready') }]) }
Then('the parcel status is verified') { |state| register(callback: -> { check.call(state.parcel).to_be('ready') }) }
Then('the parcel status is now verified') { |state| register(callback: -> { check.call(state.parcel).to_be('idle') }) }
Then('the switch position is checked') { |state| register(-> { check.call(state.switch).to_be('on') }) }
Then('the switch position is now checked') { |state| register(-> { check.call(state.switch).not.to_be('on') }) }
Then('the warehouse is inspected') { |state| register(-> { (->(value) { save(value) }).call(external) }) }
Then('the warehouse is now inspected') { |state| register(-> { (proc { |value| save(value) }).call(external) }) }
