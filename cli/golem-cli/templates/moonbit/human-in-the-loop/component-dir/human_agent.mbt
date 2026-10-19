///|
/// HumanAgent makes decisions on workflow approval requests
#derive.agent
#derive.mount("/humans/{username}")
struct HumanAgent {
  username : String
  pending : Map[String, ApprovalPromiseId]
}

///|
/// Creates a new human agent with the given username
fn HumanAgent::new(username : String) -> HumanAgent {
  { username, pending: {}, }
}

///|
/// Stores a pending approval request from a workflow
pub fn HumanAgent::request_approval(
  self : Self,
  workflow_id : String,
  promise_id : ApprovalPromiseId,
) -> String {
  self.pending[workflow_id] = promise_id
  "User \{self.username} received approval request for workflow \{workflow_id}"
}

///|
/// Lists all workflows that are waiting for this human's approval
#derive.endpoint(get="/pending")
pub fn HumanAgent::list_pending_approvals(self : Self) -> Array[String] {
  self.pending.keys().to_array()
}

///|
/// Makes a decision on a workflow approval request
#derive.prompt_hint("Approve or reject a workflow")
#derive.endpoint(post="/decisions")
pub fn HumanAgent::decide_approval(
  self : Self,
  workflow_id : String,
  decision : String,
) -> String {
  if decision != "approved" && decision != "rejected" {
    "Received invalid approval decision \{decision}"
  } else {
    match self.pending.get(workflow_id) {
      None => "No pending request found for workflow \{workflow_id}"
      Some(promise_id) => {
        let completed = @api.complete_promise(
          promise_id.to_host(),
          @utf8.encode(decision),
        )
        self.pending.remove(workflow_id)
        if completed {
          "Workflow \{workflow_id} was \{decision} by \{self.username}"
        } else {
          "Workflow \{workflow_id} was already decided"
        }
      }
    }
  }
}

///|
fn main {

}
