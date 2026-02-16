use golem_rust::{
    agent_definition, agent_implementation, await_promise_json, complete_promise_json,
    create_promise, description, prompt, PromiseId, Schema, endpoint
};
use std::collections::HashMap;
use uuid::Uuid;

type WorkflowId = String;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Schema)]
pub enum Decision {
    Approved,
    Rejected,
}

#[agent_definition(
    mount = "/workflows",
    phantom_agent = true
)]
pub trait WorkflowAgent {
    fn new() -> Self;

    #[prompt("Start approval process")]
    #[description("Starts a workflow that requires human approval before continuing")]
    async fn start(&mut self) -> String;
}

struct WorkflowAgentImpl {
    id: WorkflowId,
}

#[agent_implementation]
impl WorkflowAgent for WorkflowAgentImpl {
    fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string()
        }
    }

    #[endpoint(post = "/")]
    async fn start(&mut self) -> String {
        // 1. Create a promise that represents waiting for human input
        let approval_promise_id = create_promise();

        // Normally you would send this ID to some UI, email, etc.
        // For demo purposes, we'll just tell you where to send the approval
        let mut approver = HumanAgentClient::get("bob".to_string());
        approver
            .request_approval(self.id.clone(), approval_promise_id.clone())
            .await;

        // 2. Pause here until the promise is completed
        let result: Decision = await_promise_json(&approval_promise_id)
            .await
            .expect("Invalid promise payload");

        // 3. Continue based on human input
        if result == Decision::Approved {
            format!("Workflow {} was approved ✅", self.id)
        } else {
            format!("Workflow {} was rejected ❌", self.id)
        }
    }
}

#[agent_definition(
    mount = "/humans/{username}"
)]
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
