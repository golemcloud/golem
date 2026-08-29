package roles.ordinary

import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation}

@agentDefinition()
trait OrdinaryAgent extends BaseAgent {
  class Id(val name: String)
  def ping(): String
}

@agentImplementation()
final class OrdinaryAgentImpl(name: String) extends OrdinaryAgent {
  override def ping(): String = name
}
