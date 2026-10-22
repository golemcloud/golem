@agentDefinition()
trait PlannerAgent extends BaseAgent:
  class Id(val topic: String)
  def plan(): Future[Plan]

@agentImplementation()
final class PlannerAgentImpl(topic: String) extends PlannerAgent:
  def plan(): Future[Plan] =
    // use a typed client generated for a Rust ResearchAgent:
    val research = ResearchAgentClient.get(topic) //@
    research.gather().map: facts =>
      // send now, without waiting for a result:
      SlackAgentClient.get("ops").post.trigger(facts.summary) //@
      // schedule a call for tomorrow:
      research.refresh.scheduleAt(inDays(1)) //@
      // discover what other agents and tools offer:
      val agents = Reflection.getAllAgentTypes().orThrow //@
        .map(a => s"${a.name} (${a.sourceLanguage})")
      val tools = Reflection.getAllToolTypes().orThrow.map(_.name)
      Plan(facts, agents, tools)
