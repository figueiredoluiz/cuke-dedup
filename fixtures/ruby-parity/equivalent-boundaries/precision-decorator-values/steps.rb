def first
  expected = 'ready'
  expect(state).to eq(expected)
end
def second
  expected = 'idle'
  expect(state).to eq(expected)
end
Then('the parcel status is verified', &method(:first))
Then('the parcel status is now verified', &method(:second))
