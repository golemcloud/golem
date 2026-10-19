use crate::decision::{Decision, WorkflowId};
use crate::human_agent::HumanAgentClient;
use golem_rust::{
    agent_definition, agent_implementation, await_promise_json, create_promise, description,
    endpoint, prompt,
};

#[agent_definition(mount = "/workflows/{workflow_id}")]
pub trait WorkflowAgent {
    fn new(workflow_id: String) -> Self;

    #[prompt("Start approval process")]
    #[description("Starts a workflow that requires human approval before continuing")]
    #[endpoint(post = "/start")]
    async fn start(&mut self, approver: String) -> String;
}

struct WorkflowAgentImpl {
    id: WorkflowId,
}

#[agent_implementation]
impl WorkflowAgent for WorkflowAgentImpl {
    fn new(workflow_id: String) -> Self {
        Self { id: workflow_id }
    }

    async fn start(&mut self, approver: String) -> String {
        // 1. Create a promise that represents waiting for human input
        let approval_promise_id = create_promise();

        // Normally you would send this ID to some UI, email, etc.
        // For demo purposes, we'll just tell you where to send the approval
        let mut approver = HumanAgentClient::get(approver);
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
