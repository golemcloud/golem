@agentDefinition(
  mount = "/sites/{project}",
  exposeFiles = Array(("/", "/site/index.html")), //@
)
trait SiteAgent extends BaseAgent:
  class Id(val project: String)
  @endpoint("POST", "/generate")
  def generate(brief: String): AgentStream[String] //@

@agentImplementation()
final class SiteAgentImpl(project: String) extends SiteAgent:
  def generate(brief: String): AgentStream[String] =
    val parts = streamPage(brief)
    var page = ""
    AgentStream.fromPull(() => parts.pull().map {
      case Some(part) =>
        page += part
        Some(part)
      case None =>
        fs.mkdirSync("/site", literal(recursive = true))
        // it's served at /sites/{project} once this call completes:
        fs.writeFileSync("/site/index.html", page) //@
        None
    })
