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

use crate::agentic::agent_registry;

use golem_wasm_ast::analysis::analysed_type::str;

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

        let agent_initiator = agent_registry::get_agent_initiator(&agent_type_name);

        if let Some(agent) = agent_initiator {
            agent.initiate(input);
            Ok(())
        } else {
            Err(AgentError::CustomError(
                golem_wasm_rpc::ValueAndType::new(
                    golem_wasm_rpc::Value::String(format!(
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
        let resolved_agent = agent_registry::get_agent_instance();

        if let Some(agent) = resolved_agent {
            Ok(agent.agent.invoke(method_name, input))
        } else {
            Err(AgentError::CustomError(
                golem_wasm_rpc::ValueAndType::new(
                    golem_wasm_rpc::Value::String("No agent instance found".to_string()),
                    str(),
                )
                .into(),
            ))
        }
    }

    fn get_definition() -> AgentType {
        let resolved_agent = agent_registry::get_agent_instance();

        if let Some(agent) = resolved_agent {
            agent.agent.get_definition()
        } else {
            panic!("No agent instance found");
        }
    }

    fn discover_agent_types() -> Result<Vec<AgentType>, AgentError> {
        Ok(agent_registry::get_all_agent_types())
    }
}

crate::golem_agentic::export_golem_agentic!(Component with_types_in crate::golem_agentic);
