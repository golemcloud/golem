type Png = UnstructuredBinary["image/png"]

@agentDefinition(mount = "/private/{team}", auth = true)
trait PrivateChartsAgent extends BaseAgent:
  class Id(val team: String)
  @endpoint("POST", "/ask")
  def ask(question: String, ctx: InvocationContext): Future[Int]
  @endpoint("GET", "/charts/{n}")
  def open(n: Int, ctx: InvocationContext): Option[Png]

@agentImplementation()
final class PrivateChartsAgentImpl(team: String)
    extends PrivateChartsAgent:
  private val charts = ArrayBuffer.empty[Chart]

  def ask(question: String, ctx: InvocationContext): Future[Int] =
    // the agent charts a signed-in user's question for that user:
    val askedBy = ctx.caller.original.ref
    answerWithChart(question).map: png =>
      charts += Chart(askedBy, png) //@
      charts.length

  def open(n: Int, ctx: InvocationContext): Option[Png] =
    val me = ctx.caller.original
    // anyone else, even signed in, gets a 404:
    charts.lift(n - 1)
      .filter(_.askedBy.exists(me.isSameAs(_))) //@
      .map(c => UnstructuredBinary.fromInline(c.png, "image/png"))
