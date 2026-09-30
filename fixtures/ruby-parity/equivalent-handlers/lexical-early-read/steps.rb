require_relative 'providers/assertions'
Then('the parcel status is verified') do |state|
  expected = 'ready'
  with_expected do |expected|
    SyntheticAssertions.expect(state).to_be(expected)
    expected = 'idle'
  end
end
Then('the parcel status is now verified') do |state|
  expected = 'ready'
  with_expected do |expected|
    SyntheticAssertions.expect(state).to_be(expected)
    expected = 'ready'
  end
end
