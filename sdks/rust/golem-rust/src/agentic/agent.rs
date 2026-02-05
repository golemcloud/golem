// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::webhook_handler::WebhookHandler;
use crate::{create_promise};
use crate::golem_agentic::exports::golem::agent::guest::{
    AgentError, AgentType, DataValue, Principal,
};
use crate::golem_agentic::golem::agent::host::parse_agent_id;

#[async_trait::async_trait(?Send)]
pub trait BaseAgent {
    /// Gets the agent ID string of this agent.
    ///
    /// The agent ID consists of the agent type name, constructor parameter values and optional
    /// phantom ID.
    fn get_agent_id(&self) -> String;

    /// Dynamically performs a method invocation on this agent
    async fn invoke(
        &mut self,
        method_name: String,
        input: DataValue,
        principal: Principal,
    ) -> Result<DataValue, AgentError>;

    /// Gets the agent type metadata of this agent
    fn get_definition(&self) -> AgentType;

    /// Gets the phantom ID of the agent
    fn phantom_id(&self) -> Option<crate::Uuid> {
        let (_, _, phantom_id) = parse_agent_id(&self.get_agent_id()).unwrap(); // Not user-provided string so we can assume it's always correct
        phantom_id.map(|id| id.into())
    }

    async fn load_snapshot_base(&mut self, bytes: Vec<u8>) -> Result<(), String>;

    async fn save_snapshot_base(&self) -> Result<Vec<u8>, String>;

    fn create_webhook() -> WebhookHandler where Self: Sized {
        let promise_id = create_promise();
        // only available data for the agent is webhook suffix (and not prefix which in CLI and available for runtime)
        // but let's assume the agent can figure the full URL including the promise_id
        // A possible host function would be given the agent ID and promise ID, return the full URL for the webhook
        // get_webhook_url(agent_type: String, webhook_suffix: String, promise_id: String) -> String
        // This is because an a can have only 1 webhook base URL (it is known to the worker service, if the agent
        // is mounted at a particular domain, and then the promise_id
        // TODO; get_webhook_url(agent_type: String, webhook_suffix: String, promise_id: String) -> String
        // this host function can be be implemented in the worker executor using proxy.
        // We should consider the possibility of top down propagation of domain and webhook prefix as part of Principal.
        // But may be this is not ideal
        let url = format!("https://todo/{}", promise_id);

        WebhookHandler::new(promise_id)
    }
}
