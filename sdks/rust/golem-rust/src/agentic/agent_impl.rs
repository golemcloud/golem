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

use crate::agentic::{
    agent_registry, get_principal, get_resolved_agent, register_initialized_agent,
};
use crate::golem_agentic::golem::agent::common::Principal;
use crate::golem_agentic::golem::agent::host::parse_agent_id;
use crate::load_snapshot::exports::golem::api::load_snapshot::Guest as LoadSnapshotGuest;
use crate::save_snapshot::exports::golem::api::save_snapshot::Guest as SaveSnapshotGuest;
use crate::schema::wit::{decode_value, encode_value_async};
use crate::{
    agentic::{
        AgentTypeName, with_agent_initiator, with_agent_instance, with_agent_instance_async,
    },
    golem_agentic::exports::golem::agent::guest::{AgentError, AgentType, Guest},
};

fn serialize_principal(p: &Principal) -> Vec<u8> {
    serde_json::to_vec(p).expect("Failed to serialize principal to JSON")
}

fn deserialize_principal(bytes: &[u8]) -> Result<Principal, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("Failed to deserialize principal: {e}"))
}

fn decode_snapshot(
    snapshot: crate::load_snapshot::exports::golem::api::load_snapshot::Snapshot,
) -> Result<(Principal, Vec<u8>), String> {
    let bytes = snapshot.payload;
    let is_json = snapshot.mime_type == "application/json";

    if bytes.is_empty() {
        return Err("Snapshot is empty".into());
    }

    if is_json {
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to parse JSON snapshot: {e}"))?;
        let version = json
            .get("version")
            .ok_or_else(|| "JSON snapshot missing 'version' field".to_string())?;
        if version.as_u64() != Some(1) {
            return Err("JSON snapshot version must be 1".to_string());
        }
        let principal = json
            .get("principal")
            .ok_or_else(|| "JSON snapshot missing 'principal' field".to_string())?;
        let principal = serde_json::from_value(principal.clone())
            .map_err(|e| format!("Failed to deserialize principal from JSON: {e}"))?;
        let state = json
            .get("state")
            .ok_or_else(|| "JSON snapshot missing 'state' field".to_string())?;
        let agent_snapshot = serde_json::to_vec(state)
            .map_err(|e| format!("Failed to re-serialize state from JSON snapshot: {e}"))?;
        Ok((principal, agent_snapshot))
    } else {
        let version = bytes[0];
        match version {
            1 => {
                let agent_snapshot = bytes[1..].to_vec();
                let principal = get_principal().unwrap_or(Principal::Anonymous);
                Ok((principal, agent_snapshot))
            }
            2 => {
                if bytes.len() < 5 {
                    return Err("Version 2 snapshot too short for principal length".into());
                }
                let principal_len = u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
                let principal_start = 5usize;
                let principal_end = principal_start
                    .checked_add(principal_len)
                    .ok_or_else(|| "Version 2 snapshot principal length overflow".to_string())?;
                if bytes.len() < principal_end {
                    return Err("Version 2 snapshot too short for principal data".into());
                }
                let principal = deserialize_principal(&bytes[principal_start..principal_end])?;
                let agent_snapshot = bytes[principal_end..].to_vec();
                Ok((principal, agent_snapshot))
            }
            _ => Err(format!("Unsupported snapshot version: {}", version)),
        }
    }
}

struct ParsedRestoreIdentity {
    agent_type: String,
    parameters: crate::SchemaValue,
    phantom_id: Option<crate::Uuid>,
}

fn parse_restore_identity(id: &str) -> Result<ParsedRestoreIdentity, String> {
    let (agent_type, parameters, phantom_id) = parse_agent_id(id).map_err(|e| e.to_string())?;
    let parameters = crate::decode_typed_schema_value(&parameters)
        .map_err(|e| e.to_string())?
        .into_parts()
        .1;
    Ok(ParsedRestoreIdentity {
        agent_type,
        parameters,
        phantom_id: phantom_id.map(Into::into),
    })
}

async fn load_agent_snapshot(
    snapshot: crate::load_snapshot::exports::golem::api::load_snapshot::Snapshot,
    id: &str,
    parse_identity: impl FnOnce(&str) -> Result<ParsedRestoreIdentity, String>,
) -> Result<(), String> {
    let (principal, agent_snapshot) = decode_snapshot(snapshot)?;
    let identity = parse_identity(id)?;
    let agent_type_name = AgentTypeName(identity.agent_type.clone());
    let context = crate::agentic::SnapshotRestoreContext {
        principal: principal.clone(),
        agent_type: identity.agent_type,
        parameters: identity.parameters,
        phantom_id: identity.phantom_id,
    };
    let resolved = with_agent_initiator(
        |initiator| async move { initiator.restore(agent_snapshot, context).await },
        &agent_type_name,
    )
    .await?;
    register_initialized_agent(principal, resolved);
    Ok(())
}

pub struct Component;

impl Guest for Component {
    async fn initialize(
        agent_type: String,
        input: crate::schema::wit::wire::SchemaValueTree,
        principal: Principal,
    ) -> Result<(), AgentError> {
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);

        let agent_type_name = AgentTypeName(agent_type.clone());
        let _agent_type = agent_registry::get_enriched_agent_type_by_name(&agent_type_name)
            .unwrap_or_else(|| {
                let agent_types = agent_registry::get_all_agent_types();
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

        let input = decode_value(input)
            .map_err(|e| AgentError::InvalidInput(format!("invalid schema value input: {e}")))?;

        let commit_principal = principal.clone();
        let resolved = with_agent_initiator(
            |initiator| async move { initiator.initiate(input, principal).await },
            &agent_type_name,
        )
        .await?;
        register_initialized_agent(commit_principal, resolved);
        Ok(())
    }

    // https://github.com/golemcloud/golem/issues/2374#issuecomment-3618565370
    #[allow(clippy::await_holding_refcell_ref)]
    async fn invoke(
        method_name: String,
        input: crate::schema::wit::wire::SchemaValueTree,
        principal: Principal,
    ) -> Result<Option<crate::schema::wit::wire::SchemaValueTree>, AgentError> {
        {
            let agent_id = std::env::var("GOLEM_AGENT_ID")
                .expect("GOLEM_AGENT_ID environment variable must be set");
            parse_agent_id(&agent_id).map_err(|e| AgentError::InvalidInput(e.to_string()))?;
        }

        let input = decode_value(input)
            .map_err(|e| AgentError::InvalidInput(format!("invalid schema value input: {e}")))?;

        with_agent_instance_async(|resolved_agent| async move {
            let result = resolved_agent
                .agent
                .borrow_mut()
                .as_mut()
                .invoke(method_name, input, principal)
                .await?;
            match result.value {
                Some(value) => encode_value_async(&value).await.map(Some).map_err(|e| {
                    AgentError::InvalidInput(format!("invalid schema value output: {e}"))
                }),
                None => Ok(None),
            }
        })
        .await
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
    async fn load(
        snapshot: crate::load_snapshot::exports::golem::api::load_snapshot::Snapshot,
    ) -> Result<(), String> {
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);

        let agent_id = get_resolved_agent();

        if agent_id.is_some() {
            return Err("Agent is already initialized".to_string());
        }

        let id = std::env::var("GOLEM_AGENT_ID")
            .expect("GOLEM_AGENT_ID environment variable must be set");

        load_agent_snapshot(snapshot, &id, parse_restore_identity).await
    }
}

impl SaveSnapshotGuest for Component {
    // https://github.com/golemcloud/golem/issues/2374#issuecomment-3618565370
    #[allow(clippy::await_holding_refcell_ref)]
    async fn save() -> crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
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
                let data =
                    serde_json::to_vec(&envelope).expect("Failed to serialize snapshot envelope");
                crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
                    payload: data,
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
                    payload: full_snapshot,
                    mime_type: "application/octet-stream".to_string(),
                }
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsedRestoreIdentity, decode_snapshot, load_agent_snapshot, serialize_principal};
    use crate::agentic::{
        AgentInitiator, AgentInvocationResult, BaseAgent, ResolvedAgent, SnapshotData,
        SnapshotRestoreContext, get_principal, get_resolved_agent, get_state,
        register_agent_initiator, with_agent_instance_async,
    };
    use crate::golem_agentic::exports::golem::agent::guest::AgentType;
    use crate::golem_agentic::golem::agent::common::{AgentError, Principal};
    use crate::{SchemaValue, load_snapshot};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    static INITIALIZE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RESTORE_CALLS: AtomicUsize = AtomicUsize::new(0);

    struct TestAgent {
        snapshot: Vec<u8>,
    }

    #[async_trait::async_trait(?Send)]
    impl BaseAgent for TestAgent {
        fn get_agent_id(&self) -> String {
            "RestoreTest()".to_string()
        }

        async fn invoke(
            &mut self,
            _method_name: String,
            _input: SchemaValue,
            _principal: Principal,
        ) -> Result<AgentInvocationResult, AgentError> {
            unreachable!()
        }

        fn get_definition(&self) -> AgentType {
            unreachable!()
        }

        async fn save_snapshot_base(&self) -> Result<SnapshotData, String> {
            Ok(SnapshotData {
                data: self.snapshot.clone(),
                mime_type: "application/octet-stream".to_string(),
            })
        }
    }

    struct TestInitiator;

    #[async_trait::async_trait(?Send)]
    impl AgentInitiator for TestInitiator {
        async fn initiate(
            &self,
            _params: SchemaValue,
            _principal: Principal,
        ) -> Result<ResolvedAgent, AgentError> {
            INITIALIZE_CALLS.fetch_add(1, Ordering::SeqCst);
            unreachable!()
        }

        async fn restore(
            &self,
            snapshot: Vec<u8>,
            context: SnapshotRestoreContext,
        ) -> Result<ResolvedAgent, String> {
            RESTORE_CALLS.fetch_add(1, Ordering::SeqCst);
            let expected_phantom =
                crate::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
            let parameters_match = matches!(
                context.parameters,
                SchemaValue::Record { ref fields }
                    if matches!(fields.as_slice(), [SchemaValue::String(value)] if value == "constructor")
            );
            if context.agent_type != "RestoreTest"
                || !matches!(context.principal, Principal::Anonymous)
                || !parameters_match
                || context.phantom_id != Some(expected_phantom)
            {
                return Err("unexpected restore context".to_string());
            }
            if snapshot == b"fail" {
                return Err("restore failed".to_string());
            }
            Ok(ResolvedAgent::new(Box::new(TestAgent { snapshot })))
        }
    }

    fn parsed_test_identity(id: &str) -> Result<ParsedRestoreIdentity, String> {
        assert_eq!(
            id,
            "RestoreTest(\"constructor\")[00000000-0000-0000-0000-000000000001]"
        );
        Ok(ParsedRestoreIdentity {
            agent_type: "RestoreTest".to_string(),
            parameters: SchemaValue::Record {
                fields: vec![SchemaValue::String("constructor".to_string())],
            },
            phantom_id: Some(
                crate::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            ),
        })
    }

    fn binary_snapshot(
        state: &[u8],
    ) -> load_snapshot::exports::golem::api::load_snapshot::Snapshot {
        let principal_bytes = serialize_principal(&Principal::Anonymous);
        let mut payload = vec![2];
        payload.extend_from_slice(&(principal_bytes.len() as u32).to_be_bytes());
        payload.extend_from_slice(&principal_bytes);
        payload.extend_from_slice(state);
        load_snapshot::exports::golem::api::load_snapshot::Snapshot {
            payload,
            mime_type: "application/octet-stream".to_string(),
        }
    }

    #[test]
    fn snapshot_envelopes_decode_before_restoration() {
        let principal = Principal::Anonymous;
        let json = serde_json::json!({
            "version": 1,
            "principal": serde_json::to_value(&principal).unwrap(),
            "state": { "value": 42 },
        });
        let (decoded_principal, state) = decode_snapshot(
            load_snapshot::exports::golem::api::load_snapshot::Snapshot {
                payload: serde_json::to_vec(&json).unwrap(),
                mime_type: "application/json".to_string(),
            },
        )
        .unwrap();
        assert!(matches!(decoded_principal, Principal::Anonymous));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&state).unwrap(),
            json["state"]
        );

        let principal_bytes = serialize_principal(&principal);
        let mut binary = vec![2];
        binary.extend_from_slice(&(principal_bytes.len() as u32).to_be_bytes());
        binary.extend_from_slice(&principal_bytes);
        binary.extend_from_slice(b"state");
        let (decoded_principal, state) = decode_snapshot(
            load_snapshot::exports::golem::api::load_snapshot::Snapshot {
                payload: binary,
                mime_type: "application/octet-stream".to_string(),
            },
        )
        .unwrap();
        assert!(matches!(decoded_principal, Principal::Anonymous));
        assert_eq!(state, b"state");
    }

    #[test]
    fn json_snapshot_requires_complete_version_one_envelope() {
        let cases = [
            (
                serde_json::json!({ "principal": { "tag": "anonymous" }, "state": {} }),
                "JSON snapshot missing 'version' field",
            ),
            (
                serde_json::json!({ "version": 2, "principal": { "tag": "anonymous" }, "state": {} }),
                "JSON snapshot version must be 1",
            ),
            (
                serde_json::json!({ "version": 1, "state": {} }),
                "JSON snapshot missing 'principal' field",
            ),
            (
                serde_json::json!({ "version": 1, "principal": { "tag": "anonymous" } }),
                "JSON snapshot missing 'state' field",
            ),
        ];

        for (payload, expected) in cases {
            let error = decode_snapshot(
                load_snapshot::exports::golem::api::load_snapshot::Snapshot {
                    payload: serde_json::to_vec(&payload).unwrap(),
                    mime_type: "application/json".to_string(),
                },
            )
            .unwrap_err();
            assert_eq!(error, expected);
        }
    }

    #[test]
    fn binary_snapshot_rejects_truncated_envelopes() {
        for payload in [vec![2], vec![2, 0, 0, 0], vec![2, 0, 0, 0, 2, b'{']] {
            assert!(
                decode_snapshot(
                    load_snapshot::exports::golem::api::load_snapshot::Snapshot {
                        payload,
                        mime_type: "application/octet-stream".to_string(),
                    },
                )
                .is_err()
            );
        }
    }

    #[test]
    #[allow(clippy::await_holding_refcell_ref)]
    async fn restoration_commits_principal_and_instance_only_after_success() {
        *get_state().agent_instance.borrow_mut() = Default::default();
        register_agent_initiator("RestoreTest", std::sync::Arc::new(TestInitiator));
        INITIALIZE_CALLS.store(0, Ordering::SeqCst);
        RESTORE_CALLS.store(0, Ordering::SeqCst);

        let encoded_agent_id = "RestoreTest(\"constructor\")[00000000-0000-0000-0000-000000000001]";
        let failure = load_agent_snapshot(
            binary_snapshot(b"fail"),
            encoded_agent_id,
            parsed_test_identity,
        )
        .await;
        assert_eq!(failure.unwrap_err(), "restore failed");
        assert!(get_principal().is_none());
        assert!(get_resolved_agent().is_none());
        assert_eq!(INITIALIZE_CALLS.load(Ordering::SeqCst), 0);

        load_agent_snapshot(
            binary_snapshot(b"restored"),
            encoded_agent_id,
            parsed_test_identity,
        )
        .await
        .unwrap();
        assert!(matches!(get_principal(), Some(Principal::Anonymous)));
        assert!(get_resolved_agent().is_some());
        assert_eq!(INITIALIZE_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(RESTORE_CALLS.load(Ordering::SeqCst), 2);

        let saved = with_agent_instance_async(|agent| async move {
            agent.agent.borrow().save_snapshot_base().await.unwrap()
        })
        .await;
        assert_eq!(saved.data, b"restored");

        *get_state().agent_instance.borrow_mut() = Default::default();
    }
}

#[cfg(not(feature = "export_golem_agentic_tool_middleware"))]
crate::golem_agentic::export_golem_agentic!(Component with_types_in crate::golem_agentic);
#[cfg(feature = "export_golem_agentic_tool_middleware")]
crate::golem_agentic_tool_middleware::export_golem_agentic_tool_middleware!(
    Component with_types_in crate::golem_agentic_tool_middleware
);
crate::save_snapshot::export_save_snapshot!(Component with_types_in crate::save_snapshot);
crate::load_snapshot::export_load_snapshot!(Component with_types_in crate::load_snapshot);
