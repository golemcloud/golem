package component_name

import golem.runtime.annotations.{agentDefinition, description, endpoint, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/workflows/{workflowId}")
trait WorkflowAgent extends BaseAgent {

  class Id(val workflowId: String)

  @prompt("Start approval process")
  @description("Starts a workflow that requires human approval before continuing")
  @endpoint(method = "POST", path = "/start")
  def start(approver: String): Future[String]
}
