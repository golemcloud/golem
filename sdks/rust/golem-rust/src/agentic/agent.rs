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
pub trait AgentParameterSchema {
    fn parameter_schema(
        self,
        name: &str,
        builder: &mut direct::WireSchemaBuilder,
    ) -> crate::golem_agentic::golem::agent::common::NamedField;
}

impl<T: direct::WireSchema> AgentParameterSchema for &&AgentParameterProbe<T> {
    fn parameter_schema(
        self,
        name: &str,
        builder: &mut direct::WireSchemaBuilder,
    ) -> crate::golem_agentic::golem::agent::common::NamedField {
        crate::golem_agentic::golem::agent::common::NamedField {
            name: name.to_string(),
            source: crate::golem_agentic::golem::agent::common::FieldSource::UserSupplied,
            schema: T::append_schema(builder),
            metadata: direct::empty_metadata(),
        }
    }
}

impl AgentParameterSchema for &AgentParameterProbe<Principal> {
    fn parameter_schema(
        self,
        name: &str,
        builder: &mut direct::WireSchemaBuilder,
    ) -> crate::golem_agentic::golem::agent::common::NamedField {
        use crate::golem_agentic::golem::agent::common::{
            AutoInjectedKind, FieldSource, NamedField,
        };
        NamedField {
            name: name.to_string(),
            source: FieldSource::AutoInjected(AutoInjectedKind::Principal),
            schema: builder.push(crate::schema::wit::wire::SchemaTypeBody::RecordType(
                Vec::new(),
            )),
            metadata: direct::empty_metadata(),
        }
    }
}

#[doc(hidden)]
pub trait AgentParameterStreams {
    fn parameter_contains_stream(self) -> bool;
}

impl<T: direct::WireSchema> AgentParameterStreams for &&AgentParameterProbe<T> {
    fn parameter_contains_stream(self) -> bool {
        T::contains_stream(&mut std::collections::HashSet::new())
    }
}

impl AgentParameterStreams for &AgentParameterProbe<Principal> {
    fn parameter_contains_stream(self) -> bool {
        false
    }
}

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

#[doc(hidden)]
pub struct AgentArgument<'a, T>(pub &'a T);

#[doc(hidden)]
#[allow(async_fn_in_trait)]
pub trait WriteAgentParameter {
    fn preflight_parameter(
        self,
        resources: &mut direct::WirePreflight,
    ) -> Result<(), direct::WireError>;
    async fn prepare_parameter(self) -> Result<(), direct::WireError>;
    fn write_parameter(
        self,
        writer: &mut direct::WireWriter,
    ) -> Result<Option<wire::ValueNodeIndex>, direct::WireError>;
}

impl<T: direct::IntoWire> WriteAgentParameter for &&AgentArgument<'_, T> {
    fn preflight_parameter(
        self,
        resources: &mut direct::WirePreflight,
    ) -> Result<(), direct::WireError> {
        self.0.preflight(resources)
    }
    async fn prepare_parameter(self) -> Result<(), direct::WireError> {
        self.0.prepare_wire().await
    }
    fn write_parameter(
        self,
        writer: &mut direct::WireWriter,
    ) -> Result<Option<wire::ValueNodeIndex>, direct::WireError> {
        self.0.write_wire(writer).map(Some)
    }
}

impl WriteAgentParameter for &AgentArgument<'_, Principal> {
    fn preflight_parameter(self, _: &mut direct::WirePreflight) -> Result<(), direct::WireError> {
        Ok(())
    }
    async fn prepare_parameter(self) -> Result<(), direct::WireError> {
        Ok(())
    }
    fn write_parameter(
        self,
        _: &mut direct::WireWriter,
    ) -> Result<Option<wire::ValueNodeIndex>, direct::WireError> {
        Ok(None)
    }
}

#[derive(Debug)]
pub struct SnapshotData {
    pub data: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug)]
pub struct SnapshotRestoreContext {
    pub principal: Principal,
    pub agent_type: String,
    pub parameters: wire::SchemaValueTree,
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
