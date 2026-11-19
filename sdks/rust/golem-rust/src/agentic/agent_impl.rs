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

use super::identity::golem_env_var_not_readable;
use crate::agentic::{
    InvocationContext, agent_registry, get_resolved_agent, register_initialized_agent,
};
use crate::load_snapshot::exports::golem::api::load_snapshot::Guest as LoadSnapshotGuest;
use crate::save_snapshot::exports::golem::api::save_snapshot::Guest as SaveSnapshotGuest;
use crate::{
    agentic::{
        AgentTypeName, with_agent_initiator, with_agent_instance, with_agent_instance_async,
    },
    golem_agentic::exports::golem::agent::guest::{AgentError, AgentType, Guest},
};

/// The binary snapshot version that 2.0 writes: a tag byte, then the agent's own state.
const BINARY_SNAPSHOT_VERSION: u8 = 3;

/// Reads the ID of the running agent from `GOLEM_AGENT_ID`. It panics with the card grant that the
/// agent needs when the variable is not readable.
fn agent_id_from_env() -> String {
    std::env::var("GOLEM_AGENT_ID")
        .unwrap_or_else(|_| panic!("{}", golem_env_var_not_readable("GOLEM_AGENT_ID")))
}

#[derive(serde::Deserialize)]
struct JsonSnapshotEnvelope<'a> {
    #[serde(borrow, default, deserialize_with = "present_json_field")]
    version: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow, default, deserialize_with = "present_json_field")]
    principal: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow, default, deserialize_with = "present_json_field")]
    state: Option<&'a serde_json::value::RawValue>,
}

fn present_json_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<&'de serde_json::value::RawValue>, D::Error> {
    <&serde_json::value::RawValue as serde::Deserialize>::deserialize(deserializer).map(Some)
}

fn encode_json_snapshot(state: &[u8]) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        version: u8,
        state: &'a serde_json::value::RawValue,
    }

    let state = serde_json::from_slice(state).expect("Failed to parse snapshot JSON");
    serde_json::to_vec(&Envelope { version: 2, state })
        .expect("Failed to serialize snapshot envelope")
}

/// Reads a snapshot envelope: the 2.0 forms, and the 1.6 forms that this SDK has always read.
/// The principal of a 1.6 envelope is checked as before and then dropped: the caller of an
/// invocation comes from the host, never from saved state.
fn decode_snapshot(
    snapshot: crate::load_snapshot::exports::golem::api::load_snapshot::Snapshot,
) -> Result<super::SnapshotData, String> {
    if snapshot
        .mime_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("multipart/mixed"))
    {
        return super::snapshot_parts::decode(&snapshot.payload, &snapshot.mime_type);
    }
    let bytes = snapshot.payload;
    let is_json = snapshot.mime_type == "application/json";

    if bytes.is_empty() {
        return Err("Snapshot is empty".into());
    }

    if is_json {
        let json: JsonSnapshotEnvelope<'_> = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to parse JSON snapshot: {e}"))?;
        let version = json
            .version
            .ok_or_else(|| "JSON snapshot missing 'version' field".to_string())?;
        match serde_json::from_str::<u64>(version.get()).ok() {
            Some(1) => {
                let principal = json
                    .principal
                    .ok_or_else(|| "JSON snapshot missing 'principal' field".to_string())?;
                super::principal_serde::validate_json_bytes(principal.get().as_bytes())
                    .map_err(|e| format!("Failed to deserialize principal from JSON: {e}"))?;
            }
            Some(2) => {}
            _ => return Err("JSON snapshot version must be 1 or 2".to_string()),
        }
        let state = json
            .state
            .ok_or_else(|| "JSON snapshot missing 'state' field".to_string())?;
        let agent_snapshot = state.get().as_bytes().to_vec();
        Ok(super::SnapshotData::Json(agent_snapshot))
    } else {
        let version = bytes[0];
        match version {
            1 | BINARY_SNAPSHOT_VERSION => Ok(super::SnapshotData::Bytes(bytes[1..].to_vec())),
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
                super::principal_serde::validate_json_bytes(&bytes[principal_start..principal_end])
                    .map_err(|e| format!("Failed to deserialize principal: {e}"))?;
                Ok(super::SnapshotData::Bytes(bytes[principal_end..].to_vec()))
            }
            _ => Err(format!("Unsupported snapshot version: {}", version)),
        }
    }
}

struct RestoreIdentity {
    agent_type: String,
    agent_id: crate::AgentId,
}

fn current_restore_identity() -> Result<RestoreIdentity, String> {
    let agent_type = std::env::var("GOLEM_AGENT_TYPE")
        .map_err(|_| golem_env_var_not_readable("GOLEM_AGENT_TYPE"))?;
    let agent_id = agent_id_from_env();
    let agent_id =
        crate::Uuid::parse_str(&agent_id).map_err(|e| format!("Invalid GOLEM_AGENT_ID: {e}"))?;
    Ok(RestoreIdentity {
        agent_type,
        agent_id: crate::AgentId::new(agent_id),
    })
}

async fn load_agent_snapshot(
    snapshot: crate::load_snapshot::exports::golem::api::load_snapshot::Snapshot,
    constructor: crate::schema::wit::wire::SchemaValueTree,
    identity: impl FnOnce() -> Result<RestoreIdentity, String>,
) -> Result<(), String> {
    let agent_snapshot = decode_snapshot(snapshot)?;
    let identity = identity()?;
    let agent_type_name = AgentTypeName(identity.agent_type.clone());
    let context = crate::agentic::SnapshotRestoreContext {
        agent_type: identity.agent_type,
        parameters: constructor,
        agent_id: identity.agent_id,
    };
    let resolved = with_agent_initiator(
        |initiator| async move { initiator.restore(agent_snapshot, context).await },
        &agent_type_name,
    )
    .await?;
    register_initialized_agent(resolved);
    Ok(())
}

struct AgentRuntime;

#[doc(hidden)]
pub fn install_agent_exports() {
    let _ = super::exports::AGENT.set(super::exports::AgentHooks {
        initialize: |name, input| Box::pin(AgentRuntime::initialize(name, input)),
        invoke: |name, input| Box::pin(AgentRuntime::invoke(name, input)),
        get_definition: AgentRuntime::get_definition,
        discover: AgentRuntime::discover_agent_types,
        load: |snapshot, constructor| Box::pin(AgentRuntime::load(snapshot, constructor)),
        save: || Box::pin(AgentRuntime::save()),
    });
    #[cfg(target_arch = "wasm32")]
    super::exports::raw::agent_exports::install::<AgentRuntime>();
}

impl Guest for AgentRuntime {
    async fn initialize(
        agent_type: String,
        input: crate::schema::wit::wire::SchemaValueTree,
    ) -> Result<(), AgentError> {
        let context = InvocationContext::current();
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);

        // Snapshot load reads the agent ID through libc's environment cache. Populate
        // it during recorded initialization too, before any snapshot can be saved.
        let _ = std::env::vars_os().count();

        let agent_type_name = AgentTypeName(agent_type.clone());
        if !agent_registry::has_registered_agent_type(&agent_type_name) {
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
        }

        let resolved = with_agent_initiator(
            |initiator| async move { initiator.initiate(input, context).await },
            &agent_type_name,
        )
        .await?;
        register_initialized_agent(resolved);
        Ok(())
    }

    // https://github.com/golemcloud/golem/issues/2374#issuecomment-3618565370
    #[allow(clippy::await_holding_refcell_ref)]
    async fn invoke(
        method_name: String,
        input: crate::schema::wit::wire::SchemaValueTree,
    ) -> Result<Option<crate::schema::wit::wire::SchemaValueTree>, AgentError> {
        let context = InvocationContext::current();
        crate::Uuid::parse_str(&agent_id_from_env())
            .map_err(|e| AgentError::InvalidInput(format!("Invalid GOLEM_AGENT_ID: {e}")))?;

        with_agent_instance_async(|resolved_agent| async move {
            let result = resolved_agent
                .agent
                .borrow_mut()
                .as_mut()
                .invoke(method_name, input, context)
                .await?;
            Ok(result.value)
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

impl LoadSnapshotGuest for AgentRuntime {
    // https://github.com/golemcloud/golem/issues/2374#issuecomment-3618565370
    #[allow(clippy::await_holding_refcell_ref)]
    async fn load(
        snapshot: crate::load_snapshot::exports::golem::api::load_snapshot::Snapshot,
        constructor: crate::schema::wit::wire::SchemaValueTree,
    ) -> Result<(), String> {
        wasi_logger::Logger::install().expect("failed to install wasi_logger::Logger");
        log::set_max_level(log::LevelFilter::Trace);

        let agent_id = get_resolved_agent();

        if agent_id.is_some() {
            return Err("Agent is already initialized".to_string());
        }

        load_agent_snapshot(snapshot, constructor, current_restore_identity).await
    }
}

impl SaveSnapshotGuest for AgentRuntime {
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

            match snapshot_data {
                super::SnapshotData::Multipart { state, parts } => {
                    let (payload, mime_type) = super::snapshot_parts::encode(&state, &parts)
                        .expect("Failed to encode multipart snapshot");
                    crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
                        payload,
                        mime_type,
                    }
                }
                super::SnapshotData::Json(data) => {
                    let data = encode_json_snapshot(&data);
                    crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
                        payload: data,
                        mime_type: "application/json".to_string(),
                    }
                }
                super::SnapshotData::Bytes(data) => {
                    // Custom binary snapshot: a version tag, then the agent's own state.
                    let mut full_snapshot = Vec::with_capacity(1 + data.len());
                    full_snapshot.push(BINARY_SNAPSHOT_VERSION);
                    full_snapshot.extend_from_slice(&data);
                    crate::save_snapshot::exports::golem::api::save_snapshot::Snapshot {
                        payload: full_snapshot,
                        mime_type: "application/octet-stream".to_string(),
                    }
                }
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RestoreIdentity, agent_id_from_env, decode_snapshot, encode_json_snapshot,
        golem_env_var_not_readable, load_agent_snapshot,
    };
    use crate::agentic::{
        AgentInitiator, AgentInvocationResult, BaseAgent, InvocationContext, ResolvedAgent,
        SnapshotData, SnapshotRestoreContext, get_resolved_agent, get_state,
        register_agent_initiator, with_agent_instance_async,
    };
    use crate::golem_agentic::exports::golem::agent::guest::AgentType;
    use crate::golem_agentic::golem::agent::common::AgentError;
    use crate::load_snapshot;
    use crate::schema::wit::wire::{SchemaValueNode, SchemaValueTree};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    static INITIALIZE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RESTORE_CALLS: AtomicUsize = AtomicUsize::new(0);

    struct TestAgent {
        snapshot: Vec<u8>,
        constructor: (String, u32),
    }

    #[async_trait::async_trait(?Send)]
    impl BaseAgent for TestAgent {
        fn get_agent_id(&self) -> crate::AgentId {
            restore_test_agent_id()
        }

        async fn invoke(
            &mut self,
            _method_name: String,
            _input: crate::schema::wit::wire::SchemaValueTree,
            _context: InvocationContext,
        ) -> Result<AgentInvocationResult, AgentError> {
            unreachable!()
        }

        fn get_definition(&self) -> AgentType {
            unreachable!()
        }

        async fn save_snapshot_base(&self) -> Result<SnapshotData, String> {
            let (name, count) = &self.constructor;
            let mut saved = self.snapshot.clone();
            saved.extend_from_slice(format!("|{name}|{count}").as_bytes());
            Ok(SnapshotData::Bytes(saved))
        }
    }

    struct TestInitiator;

    #[async_trait::async_trait(?Send)]
    impl AgentInitiator for TestInitiator {
        async fn initiate(
            &self,
            _params: crate::schema::wit::wire::SchemaValueTree,
            _context: InvocationContext,
        ) -> Result<ResolvedAgent, AgentError> {
            INITIALIZE_CALLS.fetch_add(1, Ordering::SeqCst);
            unreachable!()
        }

        async fn restore(
            &self,
            snapshot: SnapshotData,
            context: SnapshotRestoreContext,
        ) -> Result<ResolvedAgent, String> {
            let SnapshotData::Bytes(snapshot) = snapshot else {
                return Err("expected bytes".to_string());
            };
            RESTORE_CALLS.fetch_add(1, Ordering::SeqCst);
            if context.agent_type != "RestoreTest"
                || context.agent_id != restore_test_agent_id()
            {
                return Err("unexpected restore context".to_string());
            }
            let mut parameters = crate::agentic::DirectAgentInput::new(context.parameters)
                .map_err(|e| e.to_string())?;
            let constructor = (
                parameters.take::<String>().map_err(|e| e.to_string())?,
                parameters.take::<u32>().map_err(|e| e.to_string())?,
            );
            parameters.finish().map_err(|e| e.to_string())?;
            if snapshot == b"fail" {
                return Err("restore failed".to_string());
            }
            Ok(ResolvedAgent::new(Box::new(TestAgent {
                snapshot,
                constructor,
            })))
        }
    }

    fn constructor_tree(name: &str, count: u32) -> SchemaValueTree {
        SchemaValueTree {
            value_nodes: vec![
                SchemaValueNode::RecordValue(vec![2, 1]),
                SchemaValueNode::U32Value(count),
                SchemaValueNode::StringValue(name.to_string()),
            ],
            root: 0,
        }
    }

    fn restore_test_agent_id() -> crate::AgentId {
        crate::AgentId::new(crate::Uuid::parse_str("00000000-0000-8000-8000-000000000001").unwrap())
    }

    fn test_identity() -> Result<RestoreIdentity, String> {
        Ok(RestoreIdentity {
            agent_type: "RestoreTest".to_string(),
            agent_id: restore_test_agent_id(),
        })
    }

    fn binary_snapshot(
        state: &[u8],
    ) -> load_snapshot::exports::golem::api::load_snapshot::Snapshot {
        let mut payload = vec![BINARY_SNAPSHOT_VERSION];
        payload.extend_from_slice(state);
        load_snapshot::exports::golem::api::load_snapshot::Snapshot {
            payload,
            mime_type: "application/octet-stream".to_string(),
        }
    }

    fn snapshot(
        payload: Vec<u8>,
        mime_type: &str,
    ) -> load_snapshot::exports::golem::api::load_snapshot::Snapshot {
        load_snapshot::exports::golem::api::load_snapshot::Snapshot {
            payload,
            mime_type: mime_type.to_string(),
        }
    }

    const ANONYMOUS_1_6: &str = r#"{"tag":"anonymous"}"#;

    #[test]
    #[should_panic(
        expected = "GOLEM_AGENT_ID is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_ID"
    )]
    fn unreadable_agent_id_names_the_card_grant() {
        agent_id_from_env();
    }

    #[test]
    fn unreadable_agent_type_names_its_own_card_grant() {
        assert_eq!(
            golem_env_var_not_readable("GOLEM_AGENT_TYPE"),
            "GOLEM_AGENT_TYPE is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_TYPE"
        );
    }

    #[test]
    fn snapshot_envelopes_decode_before_restoration() {
        let json = serde_json::json!({ "version": 2, "state": { "value": 42 } });
        let SnapshotData::Json(state) = decode_snapshot(snapshot(
            serde_json::to_vec(&json).unwrap(),
            "application/json",
        ))
        .unwrap() else {
            panic!("expected JSON");
        };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&state).unwrap(),
            json["state"]
        );

        let SnapshotData::Bytes(state) = decode_snapshot(binary_snapshot(b"state")).unwrap() else {
            panic!("expected bytes");
        };
        assert_eq!(state, b"state");
    }

    #[test]
    fn every_1_6_envelope_version_still_loads_and_drops_its_principal() {
        let json = format!(r#"{{"version":1,"principal":{ANONYMOUS_1_6},"state":{{"value":42}}}}"#);
        let SnapshotData::Json(state) =
            decode_snapshot(snapshot(json.into_bytes(), "application/json")).unwrap()
        else {
            panic!("expected JSON");
        };
        assert_eq!(state, br#"{"value":42}"#);

        let mut binary_v2 = vec![2];
        binary_v2.extend_from_slice(&(ANONYMOUS_1_6.len() as u32).to_be_bytes());
        binary_v2.extend_from_slice(ANONYMOUS_1_6.as_bytes());
        binary_v2.extend_from_slice(b"state");
        let SnapshotData::Bytes(state) =
            decode_snapshot(snapshot(binary_v2, "application/octet-stream")).unwrap()
        else {
            panic!("expected bytes");
        };
        assert_eq!(state, b"state");

        let mut binary_v1 = vec![1];
        binary_v1.extend_from_slice(b"state");
        let SnapshotData::Bytes(state) =
            decode_snapshot(snapshot(binary_v1, "application/octet-stream")).unwrap()
        else {
            panic!("expected bytes");
        };
        assert_eq!(state, b"state");
    }

    #[test]
    fn a_malformed_1_6_principal_still_fails_the_load() {
        let json = br#"{"version":1,"principal":{"tag":"agent","val":{"componentId":"nope","agentId":"a"}},"state":null}"#;
        assert!(decode_snapshot(snapshot(json.to_vec(), "application/json")).is_err());

        let bad = br#"{"tag":"golem-user"}"#;
        let mut binary_v2 = vec![2];
        binary_v2.extend_from_slice(&(bad.len() as u32).to_be_bytes());
        binary_v2.extend_from_slice(bad);
        binary_v2.extend_from_slice(b"state");
        assert!(decode_snapshot(snapshot(binary_v2, "application/octet-stream")).is_err());
    }

    #[test]
    fn the_2_0_writer_has_no_principal() {
        let payload = encode_json_snapshot(br#"{"a":1}"#);
        assert_eq!(payload, br#"{"version":2,"state":{"a":1}}"#);
        assert_eq!(
            binary_snapshot(b"x").payload,
            [BINARY_SNAPSHOT_VERSION, b'x']
        );
    }

    #[test]
    fn json_snapshot_envelope_preserves_state_semantics_without_materializing_it() {
        for state in [
            "null",
            "false",
            "18446744073709551615",
            r#"[7, { "z": [null, false], "a": "árvíz" }, -12]"#,
            r#"{ "z": 3, "a": {"x": 17.5}, "text": "quote: \"" }"#,
        ] {
            let payload = encode_json_snapshot(state.as_bytes());
            let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            let envelope = serde_json::json!({
                "version": 2,
                "state": serde_json::from_str::<serde_json::Value>(state).unwrap(),
            });
            assert_eq!(parsed, envelope);
            let SnapshotData::Json(restored) =
                decode_snapshot(snapshot(payload, "application/json")).unwrap()
            else {
                panic!("expected JSON");
            };
            assert_eq!(restored, state.as_bytes());
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&restored).unwrap(),
                envelope["state"]
            );
        }
        let state = br#"{"a":1,"z":[false,null]}"#;
        assert_eq!(
            encode_json_snapshot(state),
            br#"{"version":2,"state":{"a":1,"z":[false,null]}}"#.to_vec()
        );
    }

    #[test]
    fn json_snapshot_distinguishes_null_fields_and_requires_integer_version() {
        for version in ["null", "1.0", "1e0", "-1", "\"1\"", "true"] {
            let payload = format!(
                r#"{{"version":{version},"principal":{{"tag":"anonymous"}},"state":null}}"#
            );
            let error = decode_snapshot(
                load_snapshot::exports::golem::api::load_snapshot::Snapshot {
                    payload: payload.into_bytes(),
                    mime_type: "application/json".into(),
                },
            )
            .unwrap_err();
            assert_eq!(error, "JSON snapshot version must be 1 or 2");
        }
        let error = decode_snapshot(
            load_snapshot::exports::golem::api::load_snapshot::Snapshot {
                payload: br#"{"version":1,"principal":null,"state":null}"#.to_vec(),
                mime_type: "application/json".into(),
            },
        )
        .unwrap_err();
        assert!(error.starts_with("Failed to deserialize principal from JSON:"));
    }

    #[test]
    fn json_snapshot_requires_a_complete_envelope() {
        let cases = [
            (
                serde_json::json!({ "principal": { "tag": "anonymous" }, "state": {} }),
                "JSON snapshot missing 'version' field",
            ),
            (
                serde_json::json!({ "version": 3, "state": {} }),
                "JSON snapshot version must be 1 or 2",
            ),
            (
                serde_json::json!({ "version": 2 }),
                "JSON snapshot missing 'state' field",
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
    async fn restoration_commits_the_instance_only_after_success_and_installs_no_principal() {
        *get_state().agent_instance.borrow_mut() = Default::default();
        register_agent_initiator("RestoreTest", std::sync::Arc::new(TestInitiator));
        INITIALIZE_CALLS.store(0, Ordering::SeqCst);
        RESTORE_CALLS.store(0, Ordering::SeqCst);

        for envelope in [
            r#"[1,{"tag":"anonymous"},null]"#,
            r#"{"version":1,"principal":["anonymous",null],"state":null}"#,
            r#"{"version":1,"principal":{"tag":"agent","val":["10203040-5060-7080-9012-3456789abcde","worker(7)"]},"state":null}"#,
        ] {
            let snapshot = load_snapshot::exports::golem::api::load_snapshot::Snapshot {
                payload: format!("--b\r\nContent-Type: application/json\r\nContent-Disposition: attachment; name=\"state\"\r\n\r\n{envelope}\r\n--b--\r\n").into_bytes(),
                mime_type: "multipart/mixed; boundary=b".to_string(),
            };
            assert!(
                load_agent_snapshot(snapshot, constructor_tree("from-load", 7), || {
                    panic!("malformed metadata must fail before identity resolution")
                })
                .await
                .is_err()
            );
            assert!(get_resolved_agent().is_none());
            assert_eq!(RESTORE_CALLS.load(Ordering::SeqCst), 0);
            assert_eq!(INITIALIZE_CALLS.load(Ordering::SeqCst), 0);
        }
        let failure = load_agent_snapshot(
            binary_snapshot(b"fail"),
            constructor_tree("from-load", 7),
            test_identity,
        )
        .await;
        assert_eq!(failure.unwrap_err(), "restore failed");
        assert!(get_resolved_agent().is_none());
        assert_eq!(INITIALIZE_CALLS.load(Ordering::SeqCst), 0);

        let mismatched = SchemaValueTree {
            value_nodes: vec![
                SchemaValueNode::RecordValue(vec![1]),
                SchemaValueNode::StringValue("from-load".to_string()),
            ],
            root: 0,
        };
        assert!(
            load_agent_snapshot(binary_snapshot(b"restored"), mismatched, test_identity)
                .await
                .is_err()
        );
        assert!(get_resolved_agent().is_none());

        load_agent_snapshot(
            binary_snapshot(b"restored"),
            constructor_tree("from-load", 7),
            test_identity,
        )
        .await
        .unwrap();
        assert!(get_resolved_agent().is_some());
        assert_eq!(INITIALIZE_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(RESTORE_CALLS.load(Ordering::SeqCst), 3);

        let saved = with_agent_instance_async(|agent| async move {
            agent.agent.borrow().save_snapshot_base().await.unwrap()
        })
        .await;
        let SnapshotData::Bytes(saved) = saved else {
            panic!("expected bytes");
        };
        assert_eq!(saved, b"restored|from-load|7");

        *get_state().agent_instance.borrow_mut() = Default::default();
    }
}
