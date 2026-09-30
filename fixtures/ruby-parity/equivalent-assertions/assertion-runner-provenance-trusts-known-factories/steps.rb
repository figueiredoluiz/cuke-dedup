require_relative '../providers/meter_provider'
meter_provider = MeterProvider.method(:expect)
Given('the playwright gauge reads a first value') { || meter_provider.call(gauge()).to_be(1) }
Given('the playwright gauge reads a second value') { || meter_provider.call(gauge()).to_be(2) }
require_relative '../providers/dial_provider'
dial_provider = DialProvider.method(:expect)
Given('the vitest dial reads a first entry') { || dial_provider.call(dial()).to_be(1) }
Given('the vitest dial reads a second entry') { || dial_provider.call(dial()).to_be(2) }
require_relative '../providers/probe_provider'
probe_provider = ProbeProvider.method(:expect)
Given('the bun probe reads a first sample') { || probe_provider.call(probe()).to_be(1) }
Given('the bun probe reads a second sample') { || probe_provider.call(probe()).to_be(2) }
require_relative '../providers/chain_provider'
chain_provider = ChainProvider.method(:expect)
Given('the chai sensor reads a first reading') { || chain_provider.call(sensor()).to.equal_to(1) }
Given('the chai sensor reads a second reading') { || chain_provider.call(sensor()).to.equal_to(2) }
require_relative '../providers/unknown_provider'
unknown_provider = UnknownProvider.method(:expect)
Given('the unknown valve reads a first state') { || unknown_provider.call(valve()).to_be(1) }
Given('the unknown valve reads a second state') { || unknown_provider.call(valve()).to_be(2) }
