require_relative 'helpers'

World(ProviderHelpers)

Before do
  prepare_world()
end

Around do |_scenario, run|
  run.call
end

After do
  cleanup_world()
end

Given('shipment is ready') { write_status(:ready) }
Then('shipment is now ready') { write_status(:ready) }
Given('shipment is rejected') { write_status(:rejected) }
Then('shipment is now rejected') { write_status(:accepted) }
