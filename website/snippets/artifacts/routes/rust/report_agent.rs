#[agent_definition(mount = "/reports/{team}",
  // Golem serves these live from this agent's own file system:
  filesystem_bindings = [
    ("/", "/site/index.html"), //@
    ("/*", "/site/$1"), //@
  ])]
pub trait ReportAgent {
  fn new(team: String) -> Self;
  #[endpoint(post = "/build/{month}")] //@
  async fn build(&mut self, month: String) -> Result<(), AppError>;
}
struct ReportAgentImpl;
#[agent_implementation]
impl ReportAgent for ReportAgentImpl {
  fn new(team: String) -> Self {
    // the agent writes a front-end for its own team:
    fs::create_dir_all("/site").expect("cannot create /site");
    fs::write("/site/index.html", front_end(&team)) //@
      .expect("cannot write the front-end");
    Self
  }
  async fn build(&mut self, month: String) -> Result<(), AppError> {
    // it is served at /report.html once this call returns:
    fs::write("/site/report.html", render(&month).await)?;
    Ok(())
  }
}
