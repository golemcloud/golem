///|
/// WorkflowAgent manages an approval workflow using promises
#derive.agent
#derive.mount("/workflows/{workflow_id}")
struct WorkflowAgent {
  workflow_id : String
}

///|
/// Creates a new workflow agent with the given workflow ID
fn WorkflowAgent::new(workflow_id : String) -> WorkflowAgent {
  { workflow_id, }
}

///|
/// Starts a workflow that requires human approval before continuing
#derive.prompt_hint("Start approval process")
#derive.endpoint(post="/start")
pub async fn WorkflowAgent::start(self : Self, approver : String) -> String {
  let promise_id = @api.create_promise()
  let _ = HumanAgentClient::scoped(approver, async fn(client) {
    client.request_approval(
      self.workflow_id,
      ApprovalPromiseId::from_host(promise_id),
    )
  })
  let data = @api.await_promise(promise_id)
  let decision = @utf8.decode(data)
  if decision == "approved" {
    "Workflow \{self.workflow_id} was approved ✅"
  } else {
    "Workflow \{self.workflow_id} was rejected ❌"
  }
}
