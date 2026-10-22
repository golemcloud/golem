#derive.agent
#derive.mount("/pages/{page}")
struct PageAgent {
  page : String
  mut html : String
}

fn PageAgent::new(page : String) -> PageAgent {
  { page, html: "" }
}

#derive.endpoint(put="/")
pub fn PageAgent::publish(self : Self, html : String) -> Unit {
  self.html = html
}

#derive.endpoint(get="/")
// outgoing HTTP, RPC, key-value and blob writes trap:
// publish clears the cache when it succeeds:
// cache hits do not wake the agent:
#derive.read_only //@
pub fn PageAgent::read(self : Self) -> String {
  self.html
}
