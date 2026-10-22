#[agent_definition(mount = "/sites/{project}",
  filesystem_bindings = [("/", "/site/index.html")])] //@
pub trait SiteAgent {
  fn new(project: String) -> Self;
  #[endpoint(post = "/generate")]
  fn generate(&self, brief: String) -> AgentStream<String>; //@
}
struct SiteAgentImpl;
#[agent_implementation]
impl SiteAgent for SiteAgentImpl {
  fn new(_project: String) -> Self { Self }
  fn generate(&self, brief: String) -> AgentStream<String> {
    let (mut out, stream) = AgentStream::new();
    spawn_local(async move {
      let mut page = String::new();
      let mut parts = stream_page(&brief);
      while let Some(part) = parts.next().await {
        page += &part;
        if out.write_one(part).await.is_err() { return; }
      }
      fs::create_dir_all("/site").expect("cannot create /site");
      // it's served at /sites/{project} once this call completes:
      fs::write("/site/index.html", page).expect("write failed"); //@
    });
    stream
  }
}
