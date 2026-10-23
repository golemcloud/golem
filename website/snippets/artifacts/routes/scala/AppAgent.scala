// AppAgent's model calls this tool to save an app it just wrote:
@toolDefinition(name = "present", requiresFilesystem = true)
trait Present:
  def present(name: String, html: String): Either[BadName, String]

@toolImplementation()
final class PresentImpl extends Present:
  def present(name: String, html: String): Either[BadName, String] =
    if !Name.matches(name) then Left(BadName.Invalid)
    else
      // the app is saved in the calling agent's own files:
      fs.mkdirSync("/out", literal(recursive = true))
      fs.writeFileSync(s"/out/$name.html", html) //@
      Right(s"$name.html")

@agentDefinition(
  mount = "/apps/{chat}",
  exposeFiles = Array(("/*", "/out/$1")),
)
trait AppAgent extends BaseAgent:
  class Id(val chat: String)
  // the app's buttons fetch('sales?region=...') from here:
  @endpoint("GET", "/sales?region={region}") //@
  def sales(region: String): Future[List[Row]]

@agentImplementation()
final class AppAgentImpl(chat: String) extends AppAgent:
  // each click runs this as durable code in the agent's own VM:
  def sales(region: String): Future[List[Row]] = querySales(region) //@
