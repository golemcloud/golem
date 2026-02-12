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

use crate::agentic::{agent_registry, get_principal, get_resolved_agent, register_principal};
use crate::golem_agentic::golem::agent::host::parse_agent_id;
use crate::load_snapshot::exports::golem::api::load_snapshot::Guest as LoadSnapshotGuest;
use crate::save_snapshot::exports::golem::api::save_snapshot::Guest as SaveSnapshotGuest;
use crate::{
    agentic::{
        with_agent_initiator, with_agent_instance, with_agent_instance_async, AgentTypeName,
    },
    golem_agentic::exports::golem::agent::guest::{
        AgentError, AgentType, DataValue, Guest, Principal,
    },
};

pub struct Component;

impl Guest for Component {
    fn initialize(
        agent_type: String,
        input: DataValue,
        principal: Principal,
    ) -> Result<(), AgentError> {
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);

        let agent_types = agent_registry::get_all_agent_types();

        let agent_type = agent_types
            .iter()
            .find(|x| x.type_name == agent_type)
            .unwrap_or_else(|| {
                panic!(
                "Agent definition not found for agent name: {}. Available agents in this app is {}",
                agent_type,
                agent_types
                    .iter()
                    .map(|x| x.type_name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            });

        let agent_type_name = AgentTypeName(agent_type.type_name.clone());

        register_principal(&principal);

        with_agent_initiator(
            |initiator| async move { initiator.initiate(input, principal).await.map(|_| ()) },
            &agent_type_name,
        )
    }

    // https://github.com/golemcloud/golem/issues/2374#issuecomment-3618565370
    #[allow(clippy::await_holding_refcell_ref)]
    fn invoke(
        method_name: String,
        input: DataValue,
        principal: Principal,
    ) -> Result<DataValue, AgentError> {
        with_agent_instance_async(|resolved_agent| async move {
            resolved_agent
                .agent
                .borrow_mut()
                .as_mut()
                .invoke(method_name, input, principal)
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
    // https://github.com/golemcloud/golem/issues/2374#issuecomment-3618565370
    #[allow(clippy::await_holding_refcell_ref)]
    fn load(
        snapshot: crate::load_snapshot::exports::golem::api::load_snapshot::Snapshot,
    ) -> Result<(), String> {
        let bytes = snapshot.data;
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);

        let agent_id = get_resolved_agent();

        if agent_id.is_some() {
            return Err("Agent is already initialized".to_string());
        }

        if bytes.is_empty() {
            return Err("Snapshot is empty".into());
        }

        let version = bytes[0];

        if version != 1 {
            return Err(format!("Unsupported snapshot version: {}", version));
        }

        let agent_snapshot = bytes[1..].to_vec();

        let id = std::env::var("GOLEM_AGENT_ID")
            .expect("GOLEM_AGENT_ID environment variable must be set");

        let (agent_type_name, agent_parameters, _) =
            parse_agent_id(&id).map_err(|e| e.to_string())?;

        let principal = get_principal().expect("Failed to get initialized principal");

        with_agent_initiator(
            |initiator| async move { initiator.initiate(agent_parameters, principal).await },
            &AgentTypeName(agent_type_name),
        )
        .map_err(|e| e.to_string())?;

        with_agent_instance_async(|resolved_agent| async move {
            resolved_agent
                .agent
                .borrow_mut()
                .load_snapshot_base(agent_snapshot)
                .await
        })
    }
}

impl SaveSnapshotGuest for Component {
    // https://github.com/golemcloud/golem/issues/2374#issuecomment-3618565370
    #[allow(clippy::await_holding_refcell_ref)]
    fn save() -> crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
        with_agent_instance_async(|resolved_agent| async move {
            let agent_snapshot = resolved_agent
                .agent
                .borrow()
                .save_snapshot_base()
                .await
                .expect("Failed to save agent snapshot");

            let total_length = 1 + agent_snapshot.len();

            let mut full_snapshot = Vec::with_capacity(total_length);

            full_snapshot.push(1);

            full_snapshot.extend_from_slice(&agent_snapshot);

            crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
                data: full_snapshot,
                mime_type: "application/octet-stream".to_string(),
            }
        })
    }
}
crate::golem_agentic::export_golem_agentic!(Component with_types_in crate::golem_agentic);
crate::save_snapshot::export_save_snapshot!(Component with_types_in crate::save_snapshot);
crate::load_snapshot::export_load_snapshot!(Component with_types_in crate::load_snapshot);
