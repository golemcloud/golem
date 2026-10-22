type Png = UnstructuredBinary["image/png"]

@agentDefinition(mount = "/charts/{team}")
trait ChartAgent extends BaseAgent:
  class Id(val team: String)
  // over MCP, a chat assistant calls this when a user asks:
  @description("Draw a sales chart for one quarter")
  @endpoint("GET", "/chart.png?quarter={quarter}")
  // the reply is the PNG itself, over HTTP or MCP:
  def chart(quarter: String): Future[Png] //@

@agentImplementation()
final class ChartAgentImpl(team: String) extends ChartAgent:
  def chart(quarter: String): Future[Png] =
    // the chart is drawn for this call, and no file is written:
    querySales(team, quarter).map: rows =>
      val png = drawChart(rows) //@
      UnstructuredBinary.fromInline(png, "image/png") //@
