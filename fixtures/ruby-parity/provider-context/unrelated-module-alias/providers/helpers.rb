module ProviderHelpers
  class Formatter
    def render
      :value
    end

    alias foo render
  end
end
