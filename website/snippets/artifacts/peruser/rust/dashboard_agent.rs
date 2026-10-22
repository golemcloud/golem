// the path names the agent: /dashboards/ana is Ana's own agent:
#[agent_definition(mount = "/dashboards/{user}", //@
  filesystem_bindings = [("/*", "/out/$1")])]
pub trait DashboardAgent {
  fn new(user: String) -> Self;
  #[endpoint(post = "/ask")]
  async fn ask(&mut self, request: String)
    -> Result<String, AppError>;
}
struct DashboardAgentImpl { user: String, made: u32 }
#[agent_implementation]
impl DashboardAgent for DashboardAgentImpl {
  // the first request to /dashboards/ana creates her agent:
  fn new(user: String) -> Self { Self { user, made: 0 } } //@
  async fn ask(&mut self, request: String)
    -> Result<String, AppError> {
    self.made += 1;
    let n = self.made;
    fs::create_dir_all("/out")?;
    // the model builds this dashboard for her one request:
    fs::write(format!("/out/{n}.html"),
      dashboard_for(&self.user, &request).await?)?;
    // what it made for her is served under her path:
    Ok(format!("/dashboards/{}/{n}.html", self.user)) //@
  }
}
