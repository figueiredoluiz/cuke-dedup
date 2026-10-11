registrar = method(:Given)
registrar.call('a shared module registration') { restore_workspace() }
