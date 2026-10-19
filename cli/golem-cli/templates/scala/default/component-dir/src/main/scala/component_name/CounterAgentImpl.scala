package component_name

import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class CounterAgentImpl(@unused private val name: String) extends CounterAgent {
  private var count: Int = 0

  override def increment(): Future[Int] =
    Future.successful {
      count += 1
      count
    }

  override def value(): Future[Int] = Future.successful(count)

  override def add(by: Int): Future[Int] =
    Future.successful {
      count += by
      count
    }

  override def reset(): Future[Unit] =
    Future.successful {
      count = 0
    }
}
