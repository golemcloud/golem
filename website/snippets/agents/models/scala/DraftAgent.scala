import ai.{generateText, Output}
import aisdk.anthropic.anthropic
import aisdk.openai.openai
final case class Review(ok: Boolean, notes: String) derives Schema
final case class Draft(text: String, review: Review) derives Schema

@agentDefinition()
trait DraftAgent extends BaseAgent:
  class Id(val topic: String)
  def write(idea: String): Future[Draft]

@agentImplementation()
final class DraftAgentImpl(topic: String) extends DraftAgent:
  def write(idea: String): Future[Draft] =
    for
      // draft with one provider, review with another:
      draft <- generateText(
        model = openai("gpt-4.1"), //@
        prompt = s"Product update on $topic: $idea",
      )
      reviewed <- generateText(
        model = anthropic("claude-sonnet-4-5"), //@
        output = Output.`object`[Review],
        prompt = s"Review this product update:\n${draft.text}",
      )
    yield Draft(draft.text, reviewed.output)
