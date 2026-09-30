require_relative '../providers/assertions'
check = SyntheticAssertions.api.fetch(:expect)
alias_check = check
Then('the settings panel shows the primary account field') { |state, expected| check.call(AccountForm.new(state).primary_input).to_be(expected) }
Then('the settings panel shows the secondary account field') { |state, expected| check.call(AccountForm.new(state).secondary_input).to_be(expected) }
Then('the navigation drawer state indicator is collapsed') { |state, expected| check.call(Navigation.new(state).drawer).to_have_class(/collapsed/) }
Then('the navigation drawer state indicator is expanded') { |state, expected| check.call(Navigation.new(state).drawer).not.to_have_class(/collapsed/) }
Then('the account badge is visible') do |state, expected|
  check.call(AccountPage.new(state).badge).to_be_visible
end
Then('the account badge is now visible') { |state, expected| check.call(AccountPage.new(state).badge).to_be_visible }
Then('the account status indicator shows the first condition') { |state, expected| check.call(AccountPage.new(state).status).to_be('ready') }
Then('the account status indicator shows the final condition') { |state, expected| check.call(AccountPage.new(state).status).to_be('idle') }
Then('the profile state indicator shows the first condition') { |state, expected| alias_check.call(ProfilePage.new(state).state).to_be('ready') }
Then('the profile state indicator shows the final condition') { |state, expected| alias_check.call(ProfilePage.new(state).state).to_be('idle') }
Then('the local account status shows the first condition') { |state, expected| expected = 'ready'; check.call(AccountPage.new(state).status).to_be(expected) }
Then('the local account status shows the final condition') { |state, expected| expected = 'idle'; check.call(AccountPage.new(state).status).to_be(expected) }
class StatusHandlers
  def initialize(check)
    @check = check
  end
  def first(state)
    @check.call(state.status).to_be('ready')
  end
  def final(state)
    @check.call(state.status).to_be('idle')
  end
end
status_handlers = StatusHandlers.new(check)
Then('the decorated status shows the first condition', &status_handlers.method(:first))
Then('the decorated status shows the final condition', &status_handlers.method(:final))
