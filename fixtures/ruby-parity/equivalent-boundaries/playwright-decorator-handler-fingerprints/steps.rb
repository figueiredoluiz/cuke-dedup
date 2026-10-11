def prepare
  first_page = page
  first_page.goto('/dashboard')
end
def verify
  second_page = page
  second_page.goto('/dashboard')
end
Given('an account with dashboard access', &method(:prepare))
Then('the dashboard can be opened', &method(:verify))
