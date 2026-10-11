Before do
  Given('a step registered during a callback') { perform_late_registration() }
end

Given('shipment is ready') { write_status(:ready) }
Then('shipment is now ready') { write_status(:ready) }
