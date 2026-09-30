alias setup Given
setup('legacy Cypress registration') { seed_workspace() }
registrar = method(:Given)
registrar.call('legacy Cypress registration') { open_workspace() }
