#[agent_definition(mount = "/pages/{page}")]
pub trait PageAgent {
  fn new(page: String) -> Self;
  #[endpoint(put = "/")]
  fn publish(&mut self, html: String);
  #[endpoint(get = "/")]
  // outgoing HTTP, RPC, key-value and blob writes trap:
  // publish clears the cache when it succeeds:
  // cache hits do not wake the agent:
  #[read_only] //@
  fn read(&self) -> String;
}
struct PageAgentImpl { html: String }
#[agent_implementation]
impl PageAgent for PageAgentImpl {
  fn new(_page: String) -> Self { Self { html: String::new() } }
  fn publish(&mut self, html: String) { self.html = html; }
  fn read(&self) -> String { self.html.clone() }
}
