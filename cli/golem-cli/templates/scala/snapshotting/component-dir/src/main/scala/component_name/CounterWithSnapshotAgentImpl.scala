package component_name

import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class CounterWithSnapshotAgentImpl(@unused private val name: String)
    extends CounterWithSnapshotAgent {

  private var count: Int = 0

  override def increment(): Future[Int] =
    Future.successful {
      count += 1
      count
    }

  override def value(): Future[Int] = Future.successful(count)

  def saveSnapshot(): Future[Array[Byte]] =
    Future.successful {
      Array(
        ((count >>> 24) & 0xff).toByte,
        ((count >>> 16) & 0xff).toByte,
        ((count >>> 8) & 0xff).toByte,
        (count & 0xff).toByte
      )
    }
}

object CounterWithSnapshotAgentImpl {
  def loadSnapshot(
    bytes: Array[Byte],
    context: golem.runtime.SnapshotRestoreContext
  ): Future[CounterWithSnapshotAgentImpl] = Future.successful {
    val instance = new CounterWithSnapshotAgentImpl(context.identity[String](0))
    instance.count =
      ((bytes(0) & 0xff) << 24) |
        ((bytes(1) & 0xff) << 16) |
        ((bytes(2) & 0xff) << 8) |
        (bytes(3) & 0xff)
    instance
  }
}
