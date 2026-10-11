module LocalBuilder
  def self.method_added(name)
    (@added_methods ||= []) << name
  end

  def helper
    :local
  end
end
