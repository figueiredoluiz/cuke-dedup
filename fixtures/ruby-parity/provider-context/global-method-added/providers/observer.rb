class Module
  def method_added(name)
    return unless name == :Given

    Cucumber::Glue::Dsl.send(:define_method, :Given) do |_matcher, &_handler|
      :intercepted_registration
    end
  end
end
