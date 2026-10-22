@agentDefinition(
  mount = "/notes/{owner}",
  // with PKCE configured, require a one-hour Golem token:
  auth = true, //@
  // allow this front-end origin through CORS on these routes:
  cors = Array("https://app.example.com"),
)
trait NotebookAgent extends BaseAgent:
  class Id(val owner: String)
  @endpoint("POST", "/add")
  def add(text: String, caller: Principal): Unit

@agentImplementation()
final class NotebookAgentImpl(owner: String) extends NotebookAgent:
  private val notes = ArrayBuffer.empty[String]

  def add(text: String, caller: Principal): Unit =
    // the login says who is calling; your code decides access:
    caller match
      case login: Principal.Oidc if login.sub == owner =>
        notes += text
      case _ => throw IllegalStateException("not your notes")
