package roles.combined

import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation, toolDefinition, toolMiddleware}
import golem.tool.ToolInvokeError

import scala.concurrent.Future

@agentDefinition()
trait CombinedAgent extends BaseAgent {
  class Id(val name: String)
  def ping(): String
}

@agentImplementation()
final class CombinedAgentImpl(name: String) extends CombinedAgent {
  override def ping(): String = name
}

@toolDefinition(name = "scala-combined-ping", version = "1.0.0")
trait CombinedPingTool {
  def ping(value: String): String
}

@toolMiddleware(name = "scala-combined-middleware")
final class CombinedPingMiddleware extends CombinedPingToolMiddleware {
  def ping(
    underlying: CombinedPingToolUnderlying,
    value: String
  ): Future[Either[ToolInvokeError[Nothing], String]] =
    underlying.ping(value)
}
