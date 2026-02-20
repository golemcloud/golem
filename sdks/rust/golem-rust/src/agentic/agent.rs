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

use crate::golem_agentic::exports::golem::agent::guest::{
    AgentError, AgentType, DataValue, Principal,
};
use crate::golem_agentic::golem::agent::host::parse_agent_id;

#[derive(Debug)]
pub struct SnapshotData {
    pub data: Vec<u8>,
    pub mime_type: String,
}

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

    async fn save_snapshot_base(&self) -> Result<SnapshotData, String>;
}
