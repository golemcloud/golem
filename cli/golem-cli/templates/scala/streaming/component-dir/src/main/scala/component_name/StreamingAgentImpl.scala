package component_name

import golem.runtime.annotations.agentImplementation
import golem.schema.AgentStream

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

@agentImplementation()
final class StreamingAgentImpl(@unused private val name: String) extends StreamingAgent {
  private var cancelledProducers = 0

  private def stream[A](values: List[A]): AgentStream[A] = {
    val remaining = values.iterator
    var completed = false
    AgentStream.fromPull(
      () =>
        Future.successful {
          if (remaining.hasNext) Some(remaining.next())
          else {
            completed = true
            None
          }
        },
      () => Future.successful(if (!completed) cancelledProducers += 1)
    )
  }

  private def collect(input: AgentStream[Int], total: Int = 0): Future[Int] =
    input.pull().flatMap {
      case Some(value) => collect(input, total + value)
      case None        => Future.successful(total)
    }

  override def sum(input: AgentStream[Int]): Future[Int] =
    collect(input)

  override def produce(): Future[AgentStream[Int]] =
    Future.successful(stream(List(1, 2, 3)))

  override def transform(prefix: String, input: AgentStream[Int]): Future[AgentStream[String]] =
    Future.successful(input.map(value => s"$prefix:$value"))

  override def nested(): Future[AgentStream[AgentStream[Int]]] =
    Future.successful(stream(List(stream(List(10, 20)), stream(List(30, 40)))))

  override def recoverable(): Future[AgentStream[Either[String, Int]]] =
    Future.successful(stream(List(Right(1), Left("this item could not be produced"), Right(2))))

  override def status(): Future[String] =
    Future.successful(s"ready ($cancelledProducers cancelled producers)")
}
