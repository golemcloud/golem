val boardPage = """<button id="add">Add</button>
<script>add.onclick = () => fetch('cards', {
  method: 'POST', body: JSON.stringify({ title: 'New card' }),
})</script>"""

@agentDefinition(
  // the URL names the agent: one BoardAgent per team:
  mount = "/boards/{team}", //@
  exposeFiles = Array(("/app", "/index.html")),
)
trait BoardAgent extends BaseAgent:
  class Id(val team: String)
  // the page's Add button posts to this endpoint:
  @endpoint("POST", "/cards") //@
  def add(title: String): List[String]

@agentImplementation()
final class BoardAgentImpl(team: String) extends BoardAgent:
  private val cards = ArrayBuffer.empty[String]
  fs.writeFileSync("/index.html", boardPage)

  def add(title: String): List[String] =
    cards += title
    cards.toList
