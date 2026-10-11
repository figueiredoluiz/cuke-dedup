require_relative 'lib/unrelated_helper'
# Duplicate pair: the positive control that registrations survive the helper's dispatch.
Given('entry remains independently understood') { work() }
Given('entry remains independently understood') { other() }
