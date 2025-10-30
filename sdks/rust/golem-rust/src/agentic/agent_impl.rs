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
    agentic::agent_type_name::AgentTypeName,
    golem_agentic::exports::golem::agent::guest::{AgentError, AgentType, DataValue, Guest},
};

use crate::load_snapshot::exports::golem::api::load_snapshot::Guest as LoadSnapshotGuest;
use crate::save_snapshot::exports::golem::api::save_snapshot::Guest as SaveSnapshotGuest;

use crate::agentic::agent_registry;

use golem_wasm::analysis::analysed_type::str;

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

        // the agent_type is already lower cased kebab case
        let agent_type_name = AgentTypeName(agent_type.type_name.clone());

        let initiate_result =
            agent_registry::with_agent_initiator(&agent_type_name, |agent_initiator| {
                agent_initiator.initiate(input)
            });

        if let Some(result) = initiate_result {
            result
        } else {
            Err(AgentError::CustomError(
                golem_wasm::ValueAndType::new(
                    golem_wasm::Value::String(format!(
                        "No agent implementation found for agent definition: {}",
                        agent_type.type_name
                    )),
                    str(),
                )
                .into(),
            ))
        }
    }

    fn invoke(method_name: String, input: DataValue) -> Result<DataValue, AgentError> {
        let result = agent_registry::with_agent_instance(|resolved_agent| {
            resolved_agent.agent.invoke(method_name, input)
        });
        if let Some(result) = result {
            result
        } else {
            Err(AgentError::CustomError(
                golem_wasm::ValueAndType::new(
                    golem_wasm::Value::String("No agent instance found".to_string()),
                    str(),
                )
                .into(),
            ))
        }
    }

    fn get_definition() -> AgentType {
        let agent_type = agent_registry::with_agent_instance(|resolved_agent| {
            resolved_agent.agent.get_definition()
        });

        if let Some(agent) = agent_type {
            agent
        } else {
            panic!("No agent instance found");
        }
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
        vec![]
    }
}

crate::golem_agentic::export_golem_agentic!(Component with_types_in crate::golem_agentic);
crate::save_snapshot::export_save_snapshot!(Component with_types_in crate::save_snapshot);
crate::load_snapshot::export_load_snapshot!(Component with_types_in crate::load_snapshot);
