#[agent_definition(mount = "/chats/{chat}",
  // Golem serves files this agent writes, never half-written:
  filesystem_bindings = [("/*", "/out/$1")])] //@
pub trait AnalystAgent {
  fn new(chat: String) -> Self;
  #[endpoint(post = "/ask")]
  async fn ask(&self, question: String)
    -> Result<String, AppError>;
}
struct AnalystAgentImpl { chat: String }
#[agent_implementation]
impl AnalystAgent for AnalystAgentImpl {
  fn new(chat: String) -> Self { Self { chat } }
  async fn ask(&self, question: String)
    -> Result<String, AppError> {
    // the model calls this tool when a user asks for a graph:
    let chart = ToolDefinition { name: "chart".into(), //@
      description: Some(
        "Chart sales for a quarter and link to it".into()),
      parameters_schema: Quarter::SCHEMA.into() };
    run_tool_loop(&question, model("gpt-4.1"), chart,
      async |Quarter(q)| {
        fs::create_dir_all("/out")?;
        fs::write(format!("/out/{q}.png"), chart_png(&q).await?)?; //@
        fs::write(format!("/out/{q}.html"), page_for(&q))?; //@
        Ok(format!("/chats/{}/{q}.html", self.chat))
      }).await
  }
}
