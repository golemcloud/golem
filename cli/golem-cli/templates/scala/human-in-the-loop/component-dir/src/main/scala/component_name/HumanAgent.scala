package component_name

import golem.runtime.annotations.{agentDefinition, description, endpoint, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/humans/{username}")
trait HumanAgent extends BaseAgent {

  class Id(val username: String)

  @prompt("Approve or reject a workflow")
  @description("Makes a decision on a workflow approval request")
  @endpoint(method = "POST", path = "/decisions")
  def decide(workflowId: String, decision: String): Future[String]
}
