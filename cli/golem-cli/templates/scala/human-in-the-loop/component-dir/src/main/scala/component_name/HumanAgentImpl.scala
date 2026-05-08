package component_name

import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

@agentImplementation()
final class HumanAgentImpl(private val username: String) extends HumanAgent {

  override def decide(workflowId: String, decision: String): Future[String] =
    WorkflowAgentClient
      .get(workflowId)
      .complete(decision)
      .map { ok =>
        if (ok) s"$username decided '$decision' for workflow $workflowId"
        else s"$username failed to decide for workflow $workflowId"
      }
}
