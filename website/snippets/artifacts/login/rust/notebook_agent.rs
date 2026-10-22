#[agent_definition(mount = "/notes/{owner}",
  // with PKCE configured, require a one-hour Golem token:
  auth = true, //@
  // allow this front-end origin through CORS on these routes:
  cors = ["https://app.example.com"])]
pub trait NotebookAgent {
  fn new(owner: String) -> Self;
  #[endpoint(post = "/add")]
  fn add(&mut self, text: String, caller: Principal)
    -> Result<(), String>;
}
struct NotebookAgentImpl { owner: String, notes: Vec<String> }
#[agent_implementation]
impl NotebookAgent for NotebookAgentImpl {
  fn new(owner: String) -> Self { Self { owner, notes: vec![] } }
  fn add(&mut self, text: String, caller: Principal)
    -> Result<(), String> {
    // the login says who is calling; your code decides access:
    let owner = matches!(&caller,
      Principal::Oidc(user) if user.sub == self.owner);
    if !owner { return Err("not your notes".into()); }
    self.notes.push(text);
    Ok(())
  }
}
