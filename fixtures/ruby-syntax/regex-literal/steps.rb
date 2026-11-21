# Ruby compiles regex literals with Onigmo when it loads a file; `ruby -c` does not.
# The syntax check must reject this file for the empty range, not for another error.
Given(/^a value$/) { }
  Given(/^[b-a] range$/) { }
