package toolstreaming

import golem.BaseAgent
import golem.runtime.annotations.*
import scala.concurrent.Future

@agentDefinition()
trait ScalaMethodFailure extends BaseAgent {
  class Id(val name: String)
  def fail(): Long
  def failAsync(): Future[Long]
  def value(): Long
  def increment(): Long
  def declaredError(): Either[String, Long]
}

@agentImplementation()
final class ScalaMethodFailureImpl(name: String) extends ScalaMethodFailure {
  if (name == "fail-constructor") throw new IllegalStateException("constructor gave up")
  private var count = 0L
  override def fail(): Long = {
    count += 7L
    throw new IllegalStateException("method gave up")
  }
  override def failAsync(): Future[Long] = {
    count += 7L
    Future.failed(new IllegalStateException("method gave up"))
  }
  override def value(): Long = count
  override def increment(): Long = { count += 1L; count }
  override def declaredError(): Either[String, Long] = Left("declared error")
}
