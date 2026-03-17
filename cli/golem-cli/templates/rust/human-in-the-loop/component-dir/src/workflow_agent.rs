use crate::decision::{Decision, WorkflowId};
use crate::human_agent::HumanAgentClient;
use golem_rust::{
    agent_definition, agent_implementation, await_promise_json, create_promise, description,
    endpoint, prompt,
};
use uuid::Uuid;

#[agent_definition(mount = "/workflows", phantom_agent = true)]
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
            id: Uuid::new_v4().to_string(),
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
