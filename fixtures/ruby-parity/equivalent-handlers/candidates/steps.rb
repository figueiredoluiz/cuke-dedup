require_relative 'providers/assertions'
Then('the parcel status is verified') { |state| SyntheticAssertions.expect(state).to_be('ready') }
Then('the parcel status is now verified') { |state| SyntheticAssertions.expect(state).to_be('idle') }
Then('the archive badge is visible') do |page|
  next SyntheticAssertions.expect(page.badge).to_be_visible()
end
Then('the archive badge is now visible') { |page| SyntheticAssertions.expect(page.badge).to_be_visible() }
