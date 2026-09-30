class LocalRouter
  def Given(_matcher, &_handler)
    @registrations ||= []
    @registrations << :local
  end

  def perform
    :local
  end
end
