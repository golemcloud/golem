///|
#derive.agent
struct OrdinaryAgent {
  name : String
}

///|
fn OrdinaryAgent::new(name : String) -> OrdinaryAgent {
  { name, }
}

///|
pub fn OrdinaryAgent::get_name(self : Self) -> String {
  self.name
}

///|
pub fn OrdinaryAgent::use_ambient_tool(self : Self) -> Unit {
  let client = @tool.ToolClient::new(self.name)
  client.drop()
}

///|
fn main {

}
