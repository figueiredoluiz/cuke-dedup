module Kernel
  def Given(_matcher, &_handler)
    :kernel_method
  end
end

class Object
  def Then(_matcher, &_handler)
    :object_method
  end
end
