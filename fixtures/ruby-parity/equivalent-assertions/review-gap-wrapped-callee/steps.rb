require_relative '../providers/assertions'
check = SyntheticAssertions.api.fetch(:expect)
Then('the parcel status is verified') { |state| (check).call(state).to_be('ready') }
Then('the parcel status is now verified') { |state| ((check)).call(state).to_be('idle') }
Then('the archive badge is visible') { |state| ((check)).call(state).to_be_visible }
Then('the archive badge is now visible') { |state| check.call(state).to_be_visible }
