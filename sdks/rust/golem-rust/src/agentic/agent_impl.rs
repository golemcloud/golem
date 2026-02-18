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
use crate::golem_agentic::golem::agent::common::Principal;
use crate::golem_agentic::golem::agent::host::parse_agent_id;
use crate::load_snapshot::exports::golem::api::load_snapshot::Guest as LoadSnapshotGuest;
use crate::save_snapshot::exports::golem::api::save_snapshot::Guest as SaveSnapshotGuest;
use crate::{
    agentic::{
        with_agent_initiator, with_agent_instance, with_agent_instance_async, AgentTypeName,
    },
    golem_agentic::exports::golem::agent::guest::{AgentError, AgentType, DataValue, Guest},
};

fn serialize_principal(p: &Principal) -> Vec<u8> {
    serde_json::to_vec(p).expect("Failed to serialize principal to JSON")
}

fn deserialize_principal(bytes: &[u8]) -> Result<Principal, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("Failed to deserialize principal: {e}"))
}

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
        let is_json = snapshot.mime_type == "application/json";
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);

        let agent_id = get_resolved_agent();

        if agent_id.is_some() {
            return Err("Agent is already initialized".to_string());
        }

        if bytes.is_empty() {
            return Err("Snapshot is empty".into());
        }

        let (principal, agent_snapshot) = if is_json {
            // JSON snapshot: unwrap envelope { version, principal, state }
            let json: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Failed to parse JSON snapshot: {e}"))?;
            let principal = if let Some(p) = json.get("principal") {
                serde_json::from_value(p.clone())
                    .map_err(|e| format!("Failed to deserialize principal from JSON: {e}"))?
            } else {
                get_principal().unwrap_or(Principal::Anonymous)
            };
            let state = json
                .get("state")
                .ok_or_else(|| "JSON snapshot missing 'state' field".to_string())?;
            let agent_snapshot = serde_json::to_vec(state)
                .map_err(|e| format!("Failed to re-serialize state from JSON snapshot: {e}"))?;
            (principal, agent_snapshot)
        } else {
            // Binary snapshot with version envelope
            let version = bytes[0];
            match version {
                1 => {
                    let agent_snapshot = bytes[1..].to_vec();
                    let principal = get_principal().unwrap_or(Principal::Anonymous);
                    (principal, agent_snapshot)
                }
                2 => {
                    if bytes.len() < 5 {
                        return Err("Version 2 snapshot too short for principal length".into());
                    }
                    let principal_len =
                        u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
                    let principal_start = 5;
                    let principal_end = principal_start + principal_len;
                    if bytes.len() < principal_end {
                        return Err("Version 2 snapshot too short for principal data".into());
                    }
                    let principal =
                        deserialize_principal(&bytes[principal_start..principal_end])?;
                    let agent_snapshot = bytes[principal_end..].to_vec();
                    (principal, agent_snapshot)
                }
                _ => {
                    return Err(format!("Unsupported snapshot version: {}", version));
                }
            }
        };

        register_principal(&principal);

        let id = std::env::var("GOLEM_AGENT_ID")
            .expect("GOLEM_AGENT_ID environment variable must be set");

        let (agent_type_name, agent_parameters, _) =
            parse_agent_id(&id).map_err(|e| e.to_string())?;

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
            let snapshot_data = resolved_agent
                .agent
                .borrow()
                .save_snapshot_base()
                .await
                .expect("Failed to save agent snapshot");

            let principal = get_principal().unwrap_or(Principal::Anonymous);

            if snapshot_data.mime_type == "application/json" {
                // JSON snapshot: wrap in envelope { version, principal, state }
                let state: serde_json::Value = serde_json::from_slice(&snapshot_data.data)
                    .expect("Failed to parse snapshot JSON");
                let envelope = serde_json::json!({
                    "version": 1,
                    "principal": serde_json::to_value(&principal)
                        .expect("Failed to serialize principal"),
                    "state": state,
                });
                let data = serde_json::to_vec(&envelope)
                    .expect("Failed to serialize snapshot envelope");
                crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
                    data,
                    mime_type: "application/json".to_string(),
                }
            } else {
                // Custom binary snapshot: version 2 format with principal
                let principal_bytes = serialize_principal(&principal);
                let total_length = 1 + 4 + principal_bytes.len() + snapshot_data.data.len();
                let mut full_snapshot = Vec::with_capacity(total_length);
                full_snapshot.push(2);
                full_snapshot.extend_from_slice(&(principal_bytes.len() as u32).to_be_bytes());
                full_snapshot.extend_from_slice(&principal_bytes);
                full_snapshot.extend_from_slice(&snapshot_data.data);
                crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
                    data: full_snapshot,
                    mime_type: "application/octet-stream".to_string(),
                }
            }
        })
    }
}
crate::golem_agentic::export_golem_agentic!(Component with_types_in crate::golem_agentic);
crate::save_snapshot::export_save_snapshot!(Component with_types_in crate::save_snapshot);
crate::load_snapshot::export_load_snapshot!(Component with_types_in crate::load_snapshot);
