setup = method(:Given)
setup.call('legacy Cypress registration') { seed_workspace() }
registrar = method(:Given)
registrar.call('legacy Cypress registration') { open_workspace() }
