require_relative 'providers/helpers'
require_relative 'providers/steps'

router = LocalRouter.new
router.send(:Given, 'a router-local operation') { router.perform() }
