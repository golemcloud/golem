@agentDefinition(
  mount = "/briefs/{client}",
  // Golem serves this agent's own file at /briefs/{client}:
  exposeFiles = Array(("/", "/out/index.html")), //@
)
trait BriefAgent extends BaseAgent:
  class Id(val client: String)
  def write(notes: String): Future[Unit]

@agentImplementation()
final class BriefAgentImpl(client: String) extends BriefAgent:
  def write(notes: String): Future[Unit] =
    openai.responses.create(ResponseParams(
      model = "gpt-4.1",
      input = s"Write an HTML brief for $client:\n$notes",
    )).toFuture.map: res =>
      fs.mkdirSync("/out", literal(recursive = true))
      // it writes the page into its own files:
      // a read waits for this call, so it never gets half a page:
      fs.writeFileSync("/out/index.html", res.output_text) //@
