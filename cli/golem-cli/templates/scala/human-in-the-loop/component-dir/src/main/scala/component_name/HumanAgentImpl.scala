package component_name

import golem.HostApi
import golem.runtime.annotations.agentImplementation

import scala.collection.mutable
import scala.concurrent.Future

@agentImplementation()
final class HumanAgentImpl(private val username: String) extends HumanAgent {
  private val pending = mutable.Map.empty[String, ApprovalPromiseId]

  override def requestApproval(workflowId: String, promiseId: ApprovalPromiseId): Future[String] =
    Future.successful {
      pending.update(workflowId, promiseId)
      s"User $username received approval request for workflow $workflowId"
    }

  override def listPendingApprovals(): Future[List[String]] =
    Future.successful(pending.keys.toList)

  override def decideApproval(workflowId: String, decision: String): Future[String] =
    Future.successful {
      if (decision != "approved" && decision != "rejected")
        s"Received invalid approval decision $decision"
      else
        pending.get(workflowId) match {
          case None =>
            s"No pending request found for workflow $workflowId"
          case Some(promiseId) =>
            val completed = HostApi.completePromise(promiseId.toHost, decision.getBytes("UTF-8"))
            pending.remove(workflowId)
            if (completed) s"Workflow $workflowId was $decision by $username"
            else s"Workflow $workflowId was already decided"
        }
    }
}
