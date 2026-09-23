// Copyright 2024-2026 Golem Cloud
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

use crate::golem_agentic::exports::golem::agent::guest::{AgentError, AgentType, Principal};
use crate::golem_agentic::golem::agent::host::parse_agent_id;
use crate::schema::SchemaValue;
use crate::schema::wit::{direct, wire};

pub struct AgentInvocationResult {
    pub value: Option<wire::SchemaValueTree>,
}

#[doc(hidden)]
pub struct DirectAgentInput {
    reader: direct::WireReader,
    fields: std::vec::IntoIter<wire::ValueNodeIndex>,
}

impl DirectAgentInput {
    pub fn new(input: wire::SchemaValueTree) -> Result<Self, direct::WireError> {
        let mut reader = direct::WireReader::new(input.value_nodes);
        let wire::SchemaValueNode::RecordValue(fields) = reader.take(input.root)? else {
            return Err(direct::WireError::Shape("agent input record"));
        };
        Ok(Self {
            reader,
            fields: fields.into_iter(),
        })
    }

    pub fn take<T: direct::FromWire>(&mut self) -> Result<T, direct::WireError> {
        let index = self
            .fields
            .next()
            .ok_or(direct::WireError::Shape("agent argument"))?;
        T::read_wire(&mut self.reader, index)
    }

    pub fn finish(self) -> Result<(), direct::WireError> {
        if self.fields.len() != 0 {
            return Err(direct::WireError::Shape("extra agent arguments"));
        }
        self.reader.finish()
    }
}

#[doc(hidden)]
pub struct AgentParameterProbe<T>(pub std::marker::PhantomData<T>);

#[doc(hidden)]
pub trait ReadAgentParameter {
    type Value;
    fn read_parameter(
        self,
        input: &mut DirectAgentInput,
        principal: &Principal,
    ) -> Result<Self::Value, direct::WireError>;
}

impl<T: direct::FromWire> ReadAgentParameter for &&AgentParameterProbe<T> {
    type Value = T;

    fn read_parameter(
        self,
        input: &mut DirectAgentInput,
        _: &Principal,
    ) -> Result<T, direct::WireError> {
        input.take()
    }
}

impl ReadAgentParameter for &AgentParameterProbe<Principal> {
    type Value = Principal;

    fn read_parameter(
        self,
        _: &mut DirectAgentInput,
        principal: &Principal,
    ) -> Result<Principal, direct::WireError> {
        Ok(principal.clone())
    }
}

#[derive(Debug)]
pub struct SnapshotData {
    pub data: Vec<u8>,
    pub mime_type: String,
}

#[derive(Clone, Debug)]
pub struct SnapshotRestoreContext {
    pub principal: Principal,
    pub agent_type: String,
    pub parameters: SchemaValue,
    pub phantom_id: Option<crate::Uuid>,
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
        input: wire::SchemaValueTree,
        principal: Principal,
    ) -> Result<AgentInvocationResult, AgentError>;

    /// Gets the agent type metadata of this agent
    fn get_definition(&self) -> AgentType;

    /// Gets the phantom ID of the agent
    fn phantom_id(&self) -> Option<crate::Uuid> {
        let (_, _, phantom_id) = parse_agent_id(&self.get_agent_id()).unwrap(); // Not user-provided string so we can assume it's always correct
        phantom_id.map(|id| id.into())
    }

    async fn save_snapshot_base(&self) -> Result<SnapshotData, String>;
}
