package component_name

import golem.runtime.annotations.{agentDefinition, description, endpoint, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/snapshot-counters/{name}", snapshotting = "every(1)")
trait CounterWithSnapshotAgent extends BaseAgent {

  class Id(val name: String)

  @prompt("Increase the count by one")
  @description("Increases the count by one and returns the new value")
  @endpoint(method = "POST", path = "/increment")
  def increment(): Future[Int]
}
