///|
/// WorkflowAgent manages an approval workflow using promises
#derive.agent
#derive.mount("/workflows/{workflow_id}")
struct WorkflowAgent {
  workflow_id : String
  mut promise_oplog_idx : UInt64?
  mut decided : String?
}

///|
/// Creates a new workflow agent with the given workflow ID
fn WorkflowAgent::new(workflow_id : String) -> WorkflowAgent {
  { workflow_id, promise_oplog_idx: None, decided: None }
}

///|
/// Starts a workflow that requires human approval before continuing
#derive.prompt_hint("Start approval process")
#derive.endpoint(post="/start")
pub fn WorkflowAgent::start(self : Self) -> String {
  match self.promise_oplog_idx {
    Some(_) => "Workflow \{self.workflow_id} is already pending"
    None => {
      let promise_id = @api.create_promise()
      self.promise_oplog_idx = Some(promise_id.oplog_idx)
      self.decided = None
      "Workflow \{self.workflow_id} is now pending approval"
    }
  }
}

///|
/// Wait until the approval decision is provided, then return it
pub fn WorkflowAgent::await_outcome(self : Self) -> String {
  match self.decided {
    Some(value) => value
    None =>
      match self.promise_oplog_idx {
        None => "No pending approval"
        Some(oplog_idx) => {
          let promise_id = @api.PromiseId::{
            agent_id: @api.get_self_metadata().agent_id,
            oplog_idx,
          }
          let data = @api.await_promise(promise_id)
          let result = String::from_array(
            data.to_array().map(fn(b) { b.to_int().unsafe_to_char() }),
          )
          self.decided = Some(result)
          if result == "approved" {
            "Workflow \{self.workflow_id} was approved ✅"
          } else {
            "Workflow \{self.workflow_id} was rejected ❌"
          }
        }
      }
  }
}

///|
/// Complete the workflow decision
pub fn WorkflowAgent::complete(self : Self, decision : String) -> Bool {
  match self.promise_oplog_idx {
    None => false
    Some(oplog_idx) => {
      self.decided = Some(decision)
      let promise_id = @api.PromiseId::{
        agent_id: @api.get_self_metadata().agent_id,
        oplog_idx,
      }
      let payload = Bytes::from_array(
        decision.to_array().map(fn(c) { c.to_int().to_byte() }),
      )
      @api.complete_promise(promise_id, payload)
    }
  }
}