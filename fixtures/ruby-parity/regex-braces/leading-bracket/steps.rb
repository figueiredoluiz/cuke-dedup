Given(/\Avalue []a]\z/) { first() }
Then(%r{\Avalue []a]\z}) { second() }
# Ruby 4.0 rejects both literals: a class that opens with an unescaped `]` is empty, so the file
# never loads. tree-sitter-ruby parses it, so the file is reported unparseable and the corpus
# incomplete; its error-recovered definitions still contribute, as for any syntax error.
