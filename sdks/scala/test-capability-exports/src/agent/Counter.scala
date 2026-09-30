package capabilityfixture

import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation}

@agentDefinition("counter")
trait Counter extends BaseAgent {
  class Id(val name: String)
  def add(amount: Int): Int
}

@agentImplementation()
final class CounterImpl(name: String) extends Counter {
  private var value         = name.length
  def add(amount: Int): Int = {
    value += amount
    value
  }
}
