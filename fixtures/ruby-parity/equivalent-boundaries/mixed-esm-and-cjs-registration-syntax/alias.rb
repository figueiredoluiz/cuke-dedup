setup = method(:Given)
setup.call('a shared module registration') { prepare_workspace() }
