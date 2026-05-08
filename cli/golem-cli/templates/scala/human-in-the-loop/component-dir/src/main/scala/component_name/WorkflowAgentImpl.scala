package component_name

import golem._
import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

@agentImplementation()
final class WorkflowAgentImpl(private val workflowId: String) extends WorkflowAgent {
  private var promiseId: Option[HostApi.PromiseId] = None
  private var decided: Option[String] = None

  override def start(): Future[String] =
    Future.successful {
      promiseId match {
        case Some(_) =>
          s"Workflow $workflowId is already pending"
        case None =>
          promiseId = Some(HostApi.createPromise())
          decided = None
          s"Workflow $workflowId is now pending approval"
      }
    }

  override def awaitOutcome(): Future[String] =
    decided match {
      case Some(value) =>
        Future.successful(value)
      case None =>
        promiseId match {
          case None =>
            Future.successful("No pending approval")
          case Some(p) =>
            HostApi.awaitPromise(p).map { bytes =>
              val decision = new String(bytes.map(_.toChar))
              decided = Some(decision)
              if (decision == "approved")
                s"Workflow $workflowId was approved ✅"
              else
                s"Workflow $workflowId was rejected ❌"
            }
        }
    }

  override def complete(decision: String): Future[Boolean] =
    promiseId match {
      case None =>
        Future.successful(false)
      case Some(p) =>
        Future.successful {
          decided = Some(decision)
          HostApi.completePromise(p, decision.getBytes("UTF-8"))
        }
    }
}
