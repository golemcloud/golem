package component_name

import golem.runtime.annotations.{agentDefinition, description, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition()
trait CounterAgent extends BaseAgent {

  class Id(val name: String)

  @prompt("Increase the count by one")
  @description("Increases the count by one and returns the new value")
  def increment(): Future[Int]
}
