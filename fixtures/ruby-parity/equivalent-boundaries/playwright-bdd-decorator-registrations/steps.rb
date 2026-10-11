def prepare; seed_workspace(); end
def restore; restore_workspace(); end
Given('decorated workspace registration', &method(:prepare))
Given('decorated workspace registration', &method(:restore))
