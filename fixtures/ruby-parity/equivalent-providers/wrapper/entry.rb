def corpus_step(text, &handler)
  Given(text, &handler)
end
corpus_step('wrapped registration') { first_operation() }
corpus_step('wrapped registration') { second_operation() }

def never_called(text, &handler)
  Given(text, &handler)
end
untrusted_step = ->(text, &handler) { [text, handler] }
untrusted_step.call('ordinary helper does not register') { unrelated_operation() }
