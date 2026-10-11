Given('I am on login page') { page.goto('/login') }
Given('I am on the login page') { page.goto('/login'); page.wait_for_load_state() }
