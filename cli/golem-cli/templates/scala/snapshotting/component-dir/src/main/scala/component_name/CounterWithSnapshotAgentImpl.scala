package component_name

import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class CounterWithSnapshotAgentImpl(@unused private val name: String)
    extends CounterWithSnapshotAgent {

  private var value: Int = 0

  override def increment(): Future[Int] =
    Future.successful {
      value += 1
      value
    }

  def saveSnapshot(): Future[Array[Byte]] =
    Future.successful {
      Array(
        ((value >>> 24) & 0xff).toByte,
        ((value >>> 16) & 0xff).toByte,
        ((value >>> 8) & 0xff).toByte,
        (value & 0xff).toByte
      )
    }

  def loadSnapshot(bytes: Array[Byte]): Future[Unit] =
    Future.successful {
      value =
        ((bytes(0) & 0xff) << 24) |
          ((bytes(1) & 0xff) << 16) |
          ((bytes(2) & 0xff) << 8) |
          (bytes(3) & 0xff)
    }
}
