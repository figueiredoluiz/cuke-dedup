Given('shipment is ready') { write_status(:ready) }
Then('shipment is now ready') { write_status(:ready) }
