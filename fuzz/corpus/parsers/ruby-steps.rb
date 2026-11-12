Given('a seeded matcher') do
  work()
end

When(/^the seeded value is (\d+)$/i) { |count| work(count.to_i) }

Then('the seeded result is {int}') do |expected|
  expect(result).to eq(expected)
end
