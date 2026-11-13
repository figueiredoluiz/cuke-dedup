# Loads selected step-definition files under a recording Cucumber DSL and prints what Ruby
# registered. Mirrors Cucumber-Ruby's shape: `Cucumber::Glue::Dsl` holds
# `register_rb_step_definition`, every dialect keyword is an `alias_method` copy of it, and the
# top-level object extends the module. A registration's location is its block's, as Cucumber
# records it, or the call site for a Symbol handler. Hooks and other DSL calls are accepted and
# ignored; handler blocks never run. No gem is available, so a gem `require` fails as if the gem
# were missing. An undefined method called on the top-level object stands for a project helper
# defined elsewhere and returns an opaque value; constants stay strict.
#
# Usage: ruby --disable-gems --parser=prism ruby-load-harness.rb OUTPUT ROOT KEYWORDS_RS LOAD_PATHS_JSON FILE...
# Writes one JSON object with `registrations`, `loadErrors` and `replaced` to OUTPUT, so output a
# fixture prints cannot corrupt it.
require "json"

output = ARGV.shift
root = File.realpath(ARGV.shift)
keywords = File.read(ARGV.shift, encoding: "UTF-8").scan(/^\s+("(?:[^"\\]|\\.)*"),$/).map { |(literal)| JSON.parse(literal) }
JSON.parse(ARGV.shift).reverse_each { |path| $LOAD_PATH.unshift(File.expand_path(path, root)) }
HARNESS_PATH = File.realpath(__FILE__)
REGISTRATIONS = []
HARNESS_FLAGS = { "m" => Regexp::MULTILINE, "i" => Regexp::IGNORECASE, "x" => Regexp::EXTENDED, "n" => Regexp::NOENCODING }.freeze

module Cucumber
  module Glue
    module Dsl
      # Cucumber-Ruby's registrar: a String or Regexp pattern and `symbol || proc`, a Symbol or Proc.
      def register_rb_step_definition(pattern, symbol = nil, _options = {}, &proc)
        handler = symbol || proc
        # `===` asks the class, so an opaque helper value cannot pass as a String or a Proc.
        raise ArgumentError, "step pattern must be a String or Regexp" unless String === pattern || Regexp === pattern
        raise ArgumentError, "step handler must be a Symbol or Proc" unless Symbol === handler || Proc === handler

        caller_site = caller_locations.find { |location| (location.absolute_path || location.path) != HARNESS_PATH }
        # A block from `eval` has a synthetic file name; fall back to the call site, then to none.
        site = [Proc === handler ? handler.source_location&.first(2) : nil,
                caller_site && [caller_site.absolute_path || caller_site.path, caller_site.lineno]]
               .compact.find { |path, _| File.file?(path) }
        # Copy a String: the caller may mutate it after registering.
        REGISTRATIONS << [site, String === pattern ? pattern.dup : pattern]
        nil
      end

      %i[World Before After Around AfterStep BeforeAll AfterAll InstallPlugin AfterConfiguration ParameterType].each do |name|
        define_method(name) { |*_arguments, **_options, &_block| nil }
      end
    end
  end
end
keywords.each { |keyword| Cucumber::Glue::Dsl.send(:alias_method, keyword, :register_rb_step_definition) }
originals = keywords.to_h { |keyword| [keyword, Cucumber::Glue::Dsl.instance_method(keyword)] }
main = TOPLEVEL_BINDING.receiver
main.extend(Cucumber::Glue::Dsl)

# Answers every message with itself, so helper results can be chained and assigned.
class OpaqueValue < BasicObject
  def method_missing(*) = self

  def respond_to_missing?(*) = false

  # Usable as a Hash key; implicit conversions (`to_int`, `to_str`) stay undefined.
  def hash = __id__

  def eql?(other) = equal?(other)
end
main.define_singleton_method(:method_missing) { |*| OpaqueValue.new }

relative = ->(path) { path.delete_prefix("#{root}/") }
# Cucumber-Ruby (10.x `support_to_load`) loads `/support/env.*` files, then the other support files,
# each group in reverse path order, then the remaining definitions in order. Input is sorted.
files = ARGV.map { |file| File.realpath(file, root) }.sort_by.with_index do |path, index|
  support = path.include?("/support/")
  next [2, index] unless support

  [path.match?(%r{/support/env\..*}) ? 0 : 1, -index]
end
load_errors = []
files.each do |path|
  require path
rescue Exception => error # rubocop:disable Lint/RescueException -- a failed file must not stop the others
  line = error.backtrace_locations&.find { |location| location.absolute_path == path }&.lineno
  load_errors << { path: relative.(path), line: line, error: error.class.name, message: error.message.lines.first.to_s.strip }
end

registrations = REGISTRATIONS.map do |site, pattern|
  path = site && relative.(File.realpath(site[0]))
  if Regexp === pattern
    flags = ::HARNESS_FLAGS.select { |_, bit| pattern.options & bit != 0 }.keys.sort.join
    { path: path, line: site&.at(1), matcherKind: "regular-expression", matcher: pattern.source, matcherFlags: flags }
  else
    { path: path, line: site&.at(1), matcherKind: "cucumber-expression", matcher: pattern, matcherFlags: "" }
  end
end
# An undefined keyword counts as replaced.
replaced = keywords.reject do |keyword|
  Cucumber::Glue::Dsl.instance_method(keyword) == originals[keyword] &&
    main.method(keyword).owner == Cucumber::Glue::Dsl
rescue NameError
  false
end
File.write(output, JSON.generate({ registrations: registrations, loadErrors: load_errors, replaced: replaced }))
