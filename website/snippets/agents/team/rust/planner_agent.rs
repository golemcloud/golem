#[agent_definition]
pub trait PlannerAgent {
  fn new(topic: String) -> Self;
  async fn plan(&self) -> Result<Plan, AppError>;
}
struct PlannerAgentImpl { topic: String }
#[agent_implementation]
impl PlannerAgent for PlannerAgentImpl {
  fn new(topic: String) -> Self { Self { topic } }
  async fn plan(&self) -> Result<Plan, AppError> {
    // use a typed client generated for a Go ResearchAgent:
    let research = ResearchAgent::get(self.topic.clone())?; //@
    let facts = research.gather().await?;
    // send now, without waiting for a result:
    SlackAgent::get("ops".into())?
      .trigger_post(facts.summary.clone())?; //@
    // schedule a call for tomorrow:
    research.schedule_refresh(in_days(1))?; //@
    // discover what other agents and tools offer:
    let agents = reflection::get_all_agent_types()?.iter() //@
      .map(|a| format!("{} ({})", a.name(), a.source_language()))
      .collect();
    let tools = reflection::get_all_tool_types()?.iter()
      .map(|t| t.lookup_name().to_string()).collect();
    Ok(Plan { facts, agents, tools })
  }
}
