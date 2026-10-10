# Property: a step keyword called through a local alias of a namespace that only returns its
# arguments still makes the corpus incomplete. Ruby can rebind the local to main (`binding`,
# `binding_of_caller`), where the call registers; the TypeScript import binding it mirrors is
# immutable. Keep the block and the String pattern: without either the call cannot register.
module OrdinaryNamespace
  def self.Given(text, &handler)
    [text, handler]
  end
end
ordinary = OrdinaryNamespace
ordinary.Given('the sigma wheel locks {word}') { default_one() }
ordinary.Given('the sigma wheel locks hard') { default_two() }
