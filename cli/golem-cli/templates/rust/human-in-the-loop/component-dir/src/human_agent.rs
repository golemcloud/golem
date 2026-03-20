use crate::decision::{Decision, WorkflowId};
use golem_rust::{
    agent_definition, agent_implementation, complete_promise_json, description, endpoint, prompt,
    PromiseId,
};
use std::collections::HashMap;

#[agent_definition(mount = "/humans/{username}")]
pub trait HumanAgent {
    fn new(username: String) -> Self;

    #[prompt("Receive approval request")]
    #[description("Stores a pending approval request from a workflow")]
    async fn request_approval(&mut self, workflow_id: WorkflowId, promise_id: PromiseId) -> String;

    #[prompt("List pending approvals")]
    #[description("Lists all workflows that are waiting for this human's approval")]
    #[endpoint(get = "/pending")]
    async fn list_pending_approvals(&mut self) -> Vec<WorkflowId>;

    #[prompt("Approve or reject a workflow")]
    #[description("Makes a decision on a workflow approval request")]
    #[endpoint(post = "/decisions")]
    async fn decide_approval(&mut self, workflow_id: WorkflowId, decision: Decision) -> String;
}

struct HumanAgentImpl {
    username: String,
    pending: HashMap<WorkflowId, PromiseId>,
}

#[agent_implementation]
impl HumanAgent for HumanAgentImpl {
    fn new(username: String) -> Self {
        Self {
            username,
            pending: HashMap::new(),
        }
    }

    async fn request_approval(&mut self, workflow_id: WorkflowId, promise_id: PromiseId) -> String {
        self.pending.insert(workflow_id.clone(), promise_id);
        format!(
            "User {} received approval request for workflow {}",
            self.username, workflow_id
        )
    }

    async fn list_pending_approvals(&mut self) -> Vec<WorkflowId> {
        self.pending.keys().cloned().collect()
    }

    async fn decide_approval(&mut self, workflow_id: WorkflowId, decision: Decision) -> String {
        if let Some(promise_id) = self.pending.remove(&workflow_id) {
            let _ = complete_promise_json(&promise_id, decision.clone());
            format!(
                "Workflow {} was {:?} by {}",
                workflow_id, decision, self.username
            )
        } else {
            "No pending request found for workflow".to_string()
        }
    }
}
