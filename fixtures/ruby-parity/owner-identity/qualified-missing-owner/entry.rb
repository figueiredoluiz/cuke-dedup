def Object.const_missing(name)
  Cucumber::Glue
end
module Ghost::Dsl
  def Given(*args)
    :local
  end
end
Given('first') { write(:ready) }
Given('second') { write(:ready) }
