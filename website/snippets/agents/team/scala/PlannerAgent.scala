@agentDefinition()
trait PlannerAgent extends BaseAgent:
  class Id(val topic: String)
  def plan(): Future[Plan]

@agentImplementation()
final class PlannerAgentImpl(topic: String) extends PlannerAgent:
  def plan(): Future[Plan] =
    // wait for a typed result from a Rust ResearchAgent:
    ResearchAgentClient.get(topic).gather().map: facts => //@
      // send now, without waiting for a result:
      SlackAgentClient.get("ops").post.trigger(facts.summary) //@
      // discover agents, their methods, and available tools:
      val agents = Reflection.getAllAgentTypes().orThrow.map: a => //@
        s"${a.name}: ${a.methods.map(_.name).mkString(", ")}" //@
      val tools = Reflection.getAllToolTypes().orThrow.map(_.name) //@
      Plan(facts, agents, tools)
