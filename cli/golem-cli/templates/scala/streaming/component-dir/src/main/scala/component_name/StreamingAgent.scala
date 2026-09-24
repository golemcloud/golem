package component_name

import golem.BaseAgent
import golem.runtime.annotations.agentDefinition

import scala.concurrent.Future

@agentDefinition()
trait StreamingAgent extends BaseAgent {
  class Id(val name: String)

  def sum(input: golem.schema.AgentStream[Int]): Future[Int]
  def produce(): Future[golem.schema.AgentStream[Int]]
  def transform(
    prefix: String,
    input: golem.schema.AgentStream[Int]
  ): Future[golem.schema.AgentStream[String]]
  def nested(): Future[golem.schema.AgentStream[golem.schema.AgentStream[Int]]]
  def recoverable(): Future[golem.schema.AgentStream[Either[String, Int]]]
  def status(): Future[String]
}
