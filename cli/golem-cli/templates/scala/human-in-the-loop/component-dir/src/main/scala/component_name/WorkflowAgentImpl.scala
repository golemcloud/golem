package component_name

import golem._
import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

@agentImplementation()
final class WorkflowAgentImpl(private val workflowId: String) extends WorkflowAgent {
  override def start(approver: String): Future[String] = {
    val promiseId = HostApi.createPromise()
    for {
      _     <- HumanAgentClient.get(approver).requestApproval(workflowId, ApprovalPromiseId.fromHost(promiseId))
      bytes <- HostApi.awaitPromise(promiseId)
    } yield {
      val decision = new String(bytes, "UTF-8")
      if (decision == "approved")
        s"Workflow $workflowId was approved ✅"
      else
        s"Workflow $workflowId was rejected ❌"
    }
  }
}
