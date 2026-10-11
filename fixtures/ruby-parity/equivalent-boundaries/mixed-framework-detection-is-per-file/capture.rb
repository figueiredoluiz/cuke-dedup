setup = method(:Given)
setup.call('a shared framework registration') { page.goto('/workspace') }
