# Synthetic helper with deferred dynamic dispatch unrelated to the Cucumber DSL.
class LocalFormattingHelper
  def log(level, message)
    logger.send(level, message)
  end
end
