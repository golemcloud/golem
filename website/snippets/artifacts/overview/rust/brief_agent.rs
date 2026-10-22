#[agent_definition(mount = "/briefs/{client}",
  // Golem serves this agent's own file at /briefs/{client}:
  filesystem_bindings = [("/", "/out/index.html")])] //@
pub trait BriefAgent {
  fn new(client: String) -> Self;
  async fn write(&mut self, notes: String) -> Result<(), AppError>;
}
struct BriefAgentImpl { client: String }
#[agent_implementation]
impl BriefAgent for BriefAgentImpl {
  fn new(client: String) -> Self { Self { client } }
  async fn write(&mut self, notes: String) -> Result<(), AppError> {
    let page = text(OpenAI::send(
      OpenAiConfig::from_env()?,
      vec![user(&format!(
        "Write an HTML brief for {}:\n{notes}", self.client))],
      model("gpt-4.1"),
    ).await)?;
    fs::create_dir_all("/out")?;
    // it writes the page into its own files:
    // a read waits for this call, so it never gets half a page:
    fs::write("/out/index.html", page)?; //@
    Ok(())
  }
}
