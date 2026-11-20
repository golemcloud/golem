package buildtools

import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation}

import scala.concurrent.Future

@agentDefinition()
trait BuildToolAgent extends BaseAgent {
  class Id(val name: String)

  def describe(numbers: List[Int]): Future[String]
}

@agentImplementation()
final class BuildToolAgentImpl(private val name: String) extends BuildToolAgent {
  override def describe(numbers: List[Int]): Future[String] =
    Future.successful(s"$name:${numbers.sum}:${numbers.max}")
}
