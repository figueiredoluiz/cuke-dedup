register = FormFixture::THEN
check = FormFixture::EXPECT
register.call('the recently edited field shows its value') do |form_world|
  check.call(form_world.editor).to_have_value(form_world.entered)
end
register.call('the added field shows the entered value') do |form_world|
  check.call(form_world.editor).to_have_value(form_world.entered)
end
