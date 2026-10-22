@agentDefinition(
  mount = "/reports/{team}",
  // Golem serves these live from this agent's own file system:
  exposeFiles = Array(
    ("/", "/site/index.html"), //@
    ("/*", "/site/$1"), //@
  ),
)
trait ReportAgent extends BaseAgent:
  class Id(val team: String)
  @endpoint("POST", "/build/{month}") //@
  def build(month: String): Future[Unit]

@agentImplementation()
final class ReportAgentImpl(team: String) extends ReportAgent:
  // the agent writes a front-end for its own team:
  fs.mkdirSync("/site", literal(recursive = true))
  fs.writeFileSync("/site/index.html", frontEnd(team)) //@

  def build(month: String): Future[Unit] =
    render(month).map: html =>
      // it is served at /report.html once this call returns:
      fs.writeFileSync("/site/report.html", html)
