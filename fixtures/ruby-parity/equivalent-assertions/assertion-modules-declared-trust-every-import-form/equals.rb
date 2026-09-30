require_relative '../providers/assertions'
provider = SyntheticAssertions
api = provider.api
Given('the gamma probe records a first reading') { || api.fetch(:expect).call(probe()).to_be(1) }
Given('the gamma probe records a second reading') { || api.fetch(:expect).call(probe()).to_be(2) }
