#derive.agent
struct PlannerAgent {
  topic : String
}

fn PlannerAgent::new(topic : String) -> PlannerAgent {
  { topic, }
}

pub async fn PlannerAgent::plan(self : Self) -> Plan {
  // use a typed client generated for a Rust ResearchAgent:
  let research = @research.ResearchAgentClient::get(self.topic) //@
  defer research.drop()
  let facts = research.gather()
  // send now, without waiting for a result:
  let slack = @slack.SlackAgentClient::get("ops")
  defer slack.drop()
  slack.trigger_post(facts.summary) //@
  // schedule a call for tomorrow:
  research.schedule_refresh(in_days(1)) //@
  // discover what other agents and tools offer:
  let agents = @reflection.get_all_agent_types() //@
    .map(a => "\{a.name()} (\{a.source_language()})")
  let tools = @reflection.get_all_tool_types()
    .map(t => t.lookup_name())
  { facts, agents, tools }
}
