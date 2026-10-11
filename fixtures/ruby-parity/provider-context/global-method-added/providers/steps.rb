module DeferredProvider
  def Given(_matcher, &_handler)
    :provider_method
  end
end

Given('shipment is ready') { write_status(:ready) }
Then('shipment is now ready') { write_status(:ready) }
