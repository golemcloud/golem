type Png = UnstructuredBinary["image/png"]

@agentDefinition(mount = "/private/{team}", auth = true)
trait PrivateChartsAgent extends BaseAgent:
  class Id(val team: String)
  @endpoint("POST", "/ask")
  def ask(question: String, caller: Principal): Future[Int]
  @endpoint("GET", "/charts/{n}")
  def open(n: Int, caller: Principal): Option[Png]

@agentImplementation()
final class PrivateChartsAgentImpl(team: String)
    extends PrivateChartsAgent:
  private val charts = ArrayBuffer.empty[Chart]

  def ask(question: String, caller: Principal): Future[Int] =
    // the agent charts a signed-in user's question for that user:
    answerWithChart(question).map: png =>
      charts += Chart(askedBy = userKey(caller), png) //@
      charts.length

  def open(n: Int, caller: Principal): Option[Png] =
    // anyone else, even signed in, gets a 404:
    charts.lift(n - 1)
      .filter(_.askedBy == userKey(caller)) //@
      .map(c => UnstructuredBinary.fromInline(c.png, "image/png"))
