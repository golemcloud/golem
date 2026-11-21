package size_fixture_main

import golem.BaseAgent
import golem.config.{AgentConfig, Config}
import golem.runtime.annotations.*
import golem.concurrent.Implicits.global
import zio.blocks.schema.Schema
import scala.concurrent.Future
import golem.bridge.client.scala_streaming.ScalaStreamingClient

final case class Settings(label: String)
object Settings {
  implicit val schema: Schema[Settings] = Schema.derived
}

@agentDefinition(mount = "/counters/{name}")
trait CounterAgent extends BaseAgent with AgentConfig[Settings] {
  class Id(val name: String)
  @endpoint(method = "GET", path = "/value")
  def value(): Future[Int]
  def add(by: Int): Future[Int]
  def configured(): Future[String]
  def payloadLength(): Future[Int]
  def slice(start: Int, length: Int): Future[String]
  def dependency(start: Int, length: Int): Future[String]
  def rpcAdd(by: Int): Future[Int]
  def tool(): Future[String]
}

@agentImplementation()
final class CounterAgentImpl(name: String, config: Config[Settings]) extends CounterAgent {
  private var count = 0
  private val payload = Vector("__SIZE_PAYLOAD__").mkString

  def value(): Future[Int] = Future.successful(count)
  def add(by: Int): Future[Int] = Future.successful { count += by; count }
  def configured(): Future[String] = Future.successful(config.value.label)
  def payloadLength(): Future[Int] = Future.successful(payload.length)
  def slice(start: Int, length: Int): Future[String] = Future.successful(payload.substring(start, start + length))
  def dependency(start: Int, length: Int): Future[String] = Future.successful(ujson.write(payload.substring(start, start + length)))
  def rpcAdd(by: Int): Future[Int] = SizeHelperClient.upsert(name).add(by)
  def tool(): Future[String] = ScalaStreamingClient().plain().map {
    case Right(value) => value
    case Left(error) => throw new IllegalStateException(s"plain tool failed: $error")
  }
}

@agentDefinition()
trait SizeHelper extends BaseAgent {
  class Id(val name: String)
  def value(): Future[Int]
  def add(by: Int): Future[Int]
}

@agentImplementation()
final class SizeHelperImpl(name: String) extends SizeHelper {
  private var count = 0
  def value(): Future[Int] = Future.successful(count)
  def add(by: Int): Future[Int] = Future.successful { count += by; count }
}
