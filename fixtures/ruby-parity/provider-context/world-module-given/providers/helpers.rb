module WorldHelpers
  def Given(_matcher, &_handler)
    @local_registration_calls = true
  end
end
