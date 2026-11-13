// AppAgent's model calls this tool to save an app it just wrote:
#[tool_definition(requires_filesystem = true)]
pub trait Present {
  #[arg(name, regex = r"^[a-z0-9-]{1,40}$")]
  fn present(&self, name: String, html: String)
    -> Result<String, AppError>;
}
#[tool_implementation]
impl Present for PresentImpl {
  fn present(&self, name: String, html: String)
    -> Result<String, AppError> {
    // the app is saved in the calling agent's own files:
    fs::create_dir_all("/out")?;
    fs::write(format!("/out/{name}.html"), html)?; //@
    Ok(format!("{name}.html"))
  }
}
#[agent_definition(mount = "/apps/{chat}",
  filesystem_bindings = [("/*", "/out/$1")])]
pub trait AppAgent {
  fn new(chat: String) -> Self;
  // the app's buttons fetch('sales?region=...') from here:
  #[endpoint(get = "/sales?region={region}")] //@
  fn sales(&self, region: String) -> Rows;
}
#[agent_implementation]
impl AppAgent for AppAgentImpl {
  fn new(_chat: String) -> Self { Self }
  // each click runs this as durable code in the agent's own VM:
  fn sales(&self, region: String) -> Rows { query_sales(&region) } //@
}
