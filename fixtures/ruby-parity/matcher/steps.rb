Given('the exact shipment matcher') { load_shipment() }
Then('the exact shipment matcher') { verify_shipment() }
Given(/shipment (ready|waiting)/) { |state| inspect(state); log(:regex_a) }
Then(/shipment (ready|waiting)/) { |state| inspect(state); log(:regex_b) }
Given('shipment is ready') { shipment.inspect('ready') }
Then('shipment is now ready') { shipment.inspect('ready') }
Given('shipment is rejected') { expect(shipment).not_to eq('ready') }
Then('shipment is now rejected') { expect(shipment).to eq('ready') }
