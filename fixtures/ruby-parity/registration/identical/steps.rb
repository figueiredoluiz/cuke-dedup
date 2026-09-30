Given('a violet token') do |value|
  store.write('violet', value)
end
When('another violet token') do |value|
  store.write('violet', value)
end
