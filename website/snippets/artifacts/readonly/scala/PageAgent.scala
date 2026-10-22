@agentDefinition(mount = "/pages/{page}")
trait PageAgent extends BaseAgent:
  class Id(val page: String)
  @endpoint("PUT", "/")
  def publish(html: String): Unit
  // outgoing HTTP, RPC, key-value and blob writes trap:
  // publish clears the cache when it succeeds:
  // cache hits do not wake the agent:
  @readOnly //@
  @endpoint("GET", "/")
  def read(): String

@agentImplementation()
final class PageAgentImpl(page: String) extends PageAgent:
  private var html = ""

  def publish(html: String): Unit = this.html = html
  def read(): String = html
