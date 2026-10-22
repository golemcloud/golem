// it works on the agent's files when its binding allows it:
#[tool_definition(requires_filesystem = true)] //@
pub trait Outline { //@
  fn outline(&self, file: String) -> Result<Headings, AppError>;
}
#[tool_implementation]
impl Outline for OutlineImpl {
  // each call runs in a fresh WebAssembly VM:
  fn outline(&self, file: String) -> Result<Headings, AppError> {
    Ok(fs::read_to_string(file)?.lines()
      .filter(|l| l.starts_with('#')).map(Into::into).collect())
  }
}
#[agent_definition]
pub trait BuildAgent {
  fn new(project: String) -> Self;
  async fn build(&self) -> Result<Headings, AppError>;
}
#[agent_implementation]
impl BuildAgent for BuildAgentImpl {
  fn new(_project: String) -> Self { Self }
  async fn build(&self) -> Result<Headings, AppError> {
    // bash calls the bound git, npm, tsc and node tools:
    let run = BashClient::new().run( //@
      "git pull && npm install && tsc && node dist/build.js".into(), //@
      "/workspace/site".into(), 600).await?;
    if run.exit_code != 0 { return Err(run.stderr.into()); }
    let file = "/workspace/site/dist/report.md".to_string();
    Ok(OutlineClient::default().outline(file).await?)
  }
}
