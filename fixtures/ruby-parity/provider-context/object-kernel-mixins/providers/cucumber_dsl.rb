module Cucumber
  module Glue
    module Dsl
      def Given(_matcher, &_handler)
        :registered_by_cucumber_dsl
      end

      def Then(_matcher, &_handler)
        :registered_by_cucumber_dsl
      end
    end
  end
end

extend(Cucumber::Glue::Dsl)
