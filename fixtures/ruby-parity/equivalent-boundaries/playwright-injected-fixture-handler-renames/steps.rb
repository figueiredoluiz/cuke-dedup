Given('an authenticated session exists') do
  admin_page = page
  admin_page.goto('/secure')
end
Then('the protected dashboard is ready') do
  member_page = page
  member_page.goto('/secure')
end
