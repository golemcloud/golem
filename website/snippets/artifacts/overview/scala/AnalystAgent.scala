@agentDefinition(
  mount = "/chats/{chat}",
  // Golem serves this agent's files only when complete:
  exposeFiles = Array(("/*", "/out/$1")), //@
)
trait AnalystAgent extends BaseAgent:
  class Id(val chat: String)
  @endpoint("POST", "/ask")
  def ask(question: String): Future[String]

@agentImplementation()
final class AnalystAgentImpl(chat: String) extends AnalystAgent:
  def ask(question: String): Future[String] =
    // the model calls this tool when a user asks for a graph:
    val chart = FunctionTool(name = "chart", //@
      description = "Chart sales for a quarter and link to it",
      parameters = Quarter.schema, parse = Quarter.parse,
      function = q =>
        val key = HostApi.generateIdempotencyKey()
        val name = Uuid.toStandardString(key)
        fs.mkdirSync("/out", literal(recursive = true))
        chartPng(q).map: png =>
          fs.writeFileSync(s"/out/$name.png", png) //@
          fs.writeFileSync(s"/out/$name.html", pageFor(q, name)) //@
          s"/chats/$chat/$name.html"
    )
    val run = openai.chat.completions.runTools(ToolRunParams(
      model = "gpt-4.1", tools = Seq(chart),
      messages = Seq(ChatMessage("user", question)),
    ), RunnerOptions(maxChatCompletions = 5))
    run.finalContent().toFuture
