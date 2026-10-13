package component_name

import golem.runtime.annotations.{agentDefinition, description, endpoint, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/humans/{username}")
trait HumanAgent extends BaseAgent {

  class Id(val username: String)

  @description("Stores a pending approval request from a workflow")
  def requestApproval(workflowId: String, promiseId: ApprovalPromiseId): Future[String]

  @description("Lists all workflows that are waiting for this human's approval")
  @endpoint(method = "GET", path = "/pending")
  def listPendingApprovals(): Future[List[String]]

  @prompt("Approve or reject a workflow")
  @description("Makes a decision on a workflow approval request")
  @endpoint(method = "POST", path = "/decisions")
  def decide(workflowId: String, decision: String): Future[String]
}
