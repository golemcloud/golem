@agentDefinition(mount = "/docs/{doc}")
trait WriterAgent extends BaseAgent:
  class Id(val doc: String)
  @endpoint("POST", "/draft")
  // clients resume this Durable Stream from an offset:
  def draft(brief: String): AgentStream[String] //@

@agentImplementation()
final class WriterAgentImpl(doc: String) extends WriterAgent:
  def draft(brief: String): AgentStream[String] =
    val sections = Iterator("Summary", "Details", "Actions")
    AgentStream.fromPull: () =>
      if !sections.hasNext then Future.successful(None)
      else
        val section = sections.next()
        // completed model calls are not repeated on recovery:
        openai.responses.create(ResponseParams(
          model = "gpt-4.1",
          input = s"$brief\nWrite the $section section.",
        )).toFuture.map(reply => Some(reply.output_text)) //@
