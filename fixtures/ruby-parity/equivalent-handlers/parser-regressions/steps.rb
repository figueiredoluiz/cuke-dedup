ParameterType(name: 'duration', regexp: /\d+ seconds/, transformer: ->(value) { value })
Given('I wait {duration}') { Thread.pass() }
Given('I wait for the page') { page.wait_for_load_state() }
Given('a documented step') { open_documentation() }
When('an ordered step runs') { run_ordered_step() }
Then('a bold step works') { verify_bold_step() }
Given('value {word}') { |value| select_value(value) }
