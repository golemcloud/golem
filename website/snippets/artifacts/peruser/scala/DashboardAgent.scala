@agentDefinition(
  // the path names the agent: /dashboards/ana is Ana's own agent:
  mount = "/dashboards/{user}", //@
  auth = true,
  exposeFiles = Array(("/*", "/out/$1")),
)
trait DashboardAgent extends BaseAgent:
  class Id(val user: String)
  @endpoint("POST", "/ask")
  def ask(request: String): Future[String]

// the first request to /dashboards/ana creates her agent:
@agentImplementation()
final class DashboardAgentImpl(user: String) extends DashboardAgent: //@
  def ask(request: String): Future[String] =
    val key = HostApi.generateIdempotencyKey()
    val name = Uuid.toStandardString(key)
    fs.mkdirSync("/out", literal(recursive = true))
    // the model builds this dashboard for her one request:
    dashboardFor(user, request).map: page =>
      fs.writeFileSync(s"/out/$name.html", page)
      // what it made for her is served under her path:
      s"/dashboards/$user/$name.html" //@
