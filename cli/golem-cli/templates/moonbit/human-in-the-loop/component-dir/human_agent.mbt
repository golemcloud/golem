///|
/// HumanAgent makes decisions on workflow approval requests
#derive.agent
#derive.mount("/humans/{username}")
struct HumanAgent {
  username : String
}

///|
/// Creates a new human agent with the given username
fn HumanAgent::new(username : String) -> HumanAgent {
  { username, }
}

///|
/// Makes a decision on a workflow approval request
#derive.prompt_hint("Approve or reject a workflow")
#derive.endpoint(post="/decisions")
pub fn HumanAgent::decide(
  self : Self,
  workflow_id : String,
  decision : String,
) -> String {
  let ok = try {
    WorkflowAgentClient::scoped(workflow_id, fn(client) raise @common.AgentError {
      client.complete(decision)
    })
  } catch {
    _ => false
  }
  if ok {
    "\{self.username} decided '\{decision}' for workflow \{workflow_id}"
  } else {
    "\{self.username} failed to decide for workflow \{workflow_id}"
  }
}

///|
fn main {

}