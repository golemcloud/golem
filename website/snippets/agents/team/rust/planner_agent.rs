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
    // wait for a typed result from a TypeScript ResearchAgent:
    let facts = ResearchAgent::get(self.topic.clone())? //@
      .gather().await?; //@
    // send now, without waiting for a result:
    SlackAgent::get("ops".into())? //@
      .trigger_post(facts.summary.clone())?; //@
    // discover agents, their methods, and available tools:
    let agents = reflection::get_all_agent_types()?.iter() //@
      .map(|a| format!("{}: {}", a.name(), a.methods().iter() //@
        .map(|m| m.name()).collect::<Vec<_>>().join(", "))) //@
      .collect();
    let tools = reflection::get_all_tool_types()?.iter() //@
      .map(|t| t.lookup_name().to_string()).collect(); //@
    Ok(Plan { facts, agents, tools })
  }
}
