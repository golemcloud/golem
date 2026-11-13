#derive.agent
struct PlannerAgent {
  topic : String
}

fn PlannerAgent::new(topic : String) -> PlannerAgent {
  { topic, }
}

pub async fn PlannerAgent::plan(self : Self) -> Plan {
  // wait for a typed result from a Rust ResearchAgent:
  let facts = @research.ResearchAgentClient::get(self.topic) //@
    .gather() //@
  // send now, without waiting for a result:
  @slack.SlackAgentClient::get("ops").trigger_post(facts.summary) //@
  // discover agents, their methods, and available tools:
  let agents = @reflection.get_all_agent_types().map(a => //@
    "\{a.name()}: \{a.methods().map(m => m.name()).join(", ")}") //@
  let tools = @reflection.get_all_tool_types() //@
    .map(t => t.lookup_name()) //@
  { facts, agents, tools }
}
