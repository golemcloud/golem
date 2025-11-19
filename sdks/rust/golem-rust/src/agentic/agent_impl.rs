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

use crate::{
    agentic::{
        with_agent_initiator, with_agent_instance, with_agent_instance_async, AgentTypeName,
    },
    golem_agentic::exports::golem::agent::guest::{AgentError, AgentType, DataValue, Guest},
};

use crate::agentic::agent_registry;
use crate::bindings::golem::agent::host::make_agent_id;
use crate::load_snapshot::exports::golem::api::load_snapshot::Guest as LoadSnapshotGuest;
use crate::save_snapshot::exports::golem::api::save_snapshot::Guest as SaveSnapshotGuest;

pub struct Component;

impl Guest for Component {
    fn initialize(agent_type: String, input: DataValue) -> Result<(), AgentError> {
        let agent_types = agent_registry::get_all_agent_types();

        let agent_type = agent_types
            .iter()
            .find(|x| x.type_name == agent_type)
            .expect(
                format!(
                "Agent definition not found for agent name: {}. Available agents in this app is {}",
                agent_type,
                agent_types
                    .iter()
                    .map(|x| x.type_name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
                .as_str(),
            );

        let agent_type_name = AgentTypeName(agent_type.type_name.clone());

        with_agent_initiator(
            |initiator| async move { initiator.initiate(input).await.map(|_| ()) },
            &agent_type_name,
        )
    }

    fn invoke(method_name: String, input: DataValue) -> Result<DataValue, AgentError> {
        with_agent_instance_async(|resolved_agent| async move {
            resolved_agent
                .agent
                .borrow_mut()
                .as_mut()
                .invoke(method_name, input)
                .await
        })
    }

    fn get_definition() -> AgentType {
        with_agent_instance(|resolved_agent| {
            resolved_agent.agent.borrow().as_ref().get_definition()
        })
    }

    fn discover_agent_types() -> Result<Vec<AgentType>, AgentError> {
        Ok(agent_registry::get_all_agent_types())
    }
}

impl LoadSnapshotGuest for Component {
    fn load(_bytes: Vec<u8>) -> Result<(), String> {
        Err("Load snapshot not implemented".to_string())
    }
}

impl SaveSnapshotGuest for Component {
    fn save() -> Vec<u8> {
        with_agent_instance_async(|resolved_agent| async move {

            // TODO; just referring to TS, verify
            let agent_id_bytes = resolved_agent.agent_id.agent_id.as_bytes();

            let agent_snapshot =
                resolved_agent.agent.borrow().save_snapshot_base().await.expect("Failed to save agent snapshot");

            let total_length = 1 + 4 + agent_id_bytes.len() + agent_snapshot.len();

            let mut full_snapshot = Vec::with_capacity(total_length);

            full_snapshot.push(1u8);

            full_snapshot.extend_from_slice(&(agent_id_bytes.len() as u32).to_be_bytes());


            full_snapshot.extend_from_slice(agent_id_bytes);

            full_snapshot.extend_from_slice(&agent_snapshot);

            full_snapshot
        })
    }

}

crate::golem_agentic::export_golem_agentic!(Component with_types_in crate::golem_agentic);
crate::save_snapshot::export_save_snapshot!(Component with_types_in crate::save_snapshot);
crate::load_snapshot::export_load_snapshot!(Component with_types_in crate::load_snapshot);
