package component_name

import golem.BaseAgent
import golem.config.AgentConfig
import golem.runtime.annotations.{agentDefinition, durableStreamSlot, durableStreams, endpoint}
import zio.blocks.schema.Schema
import golem.config.Secret

import scala.concurrent.Future

final case class StreamingConfig(externalAuth: Secret[String]) derives Schema

@agentDefinition(mount = "/durable-stream-agents/{name}")
trait StreamingAgent extends BaseAgent with AgentConfig[StreamingConfig] {
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

  @endpoint(method = "PUT", path = "/echo")
  @durableStreamSlot(source = "input", slot = "input")
  @durableStreamSlot(source = "output", slot = "$result")
  @durableStreams(allowExternalWrites = true)
  def durableEcho(input: golem.schema.AgentStream[String]): Future[golem.schema.AgentStream[String]]

  def appendExternal(
    url: String,
    producerId: String,
    values: Vector[String],
    close: Boolean
  ): Future[Option[String]]

  def readExternal(url: String): Future[Vector[String]]
}
