REGISTER = method(:Given)
handler = ->(value) { work(value) }
REGISTER.call('a captured registration', &handler)

def register_step(pattern, &handler)
  Given(pattern, &handler)
end

register_step('a forwarded registration') do
  text = <<~TEXT
    seeded #{value}
  TEXT
  work(text)
end
