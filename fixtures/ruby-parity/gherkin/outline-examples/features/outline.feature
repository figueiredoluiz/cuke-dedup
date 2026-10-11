Feature: Outline rows
  Scenario Outline: Available queues
    Given the queue <queue> is ready
    When the queue is checked

    Examples: First set
      | queue   |
      | primary |

    Examples: Second set
      | queue   |
      | standby |
