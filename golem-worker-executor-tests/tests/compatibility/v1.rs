// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Serialization tests for worker information.
//!
//! The tests are assuming composability of the serializer implementation, so if a given type A has a field of type B,
//! the test for A only contains an example value of B but there exists a separate test that tests the serialization of B.

use desert_rust::BinaryCodec;
use goldenfile::Mint;
use goldenfile::differs::Differ;
use golem_api_grpc::proto::golem::worker::RawOplogEntry;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMode, OwnerKind};
use golem_common::model::caller::Caller;
use golem_common::model::component::{
    ComponentId, ComponentRevision, InstalledPlugin, PluginPriority,
};
use golem_common::model::entity::FilesystemCapability;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::oplog::{
    CreateParameters, CreateRequest, CreateRequestKind, OplogEntry, StandaloneToolInvocation,
};
use golem_common::model::tool::{
    CompiledToolBinding, ToolActivationSnapshot, ToolBindingOwner, ToolFilesystemAccess,
    ToolProvisionConfig, ToolRegistration, ToolSource,
};
use golem_common::model::tool_middleware::{
    CompiledToolMiddlewareChain, CompiledToolMiddlewareOccurrence, RegisteredToolMiddleware,
    ToolMiddlewareSource,
};
use golem_common::model::{AgentId, IdempotencyKey, InvocationId, Timestamp};
use golem_common::schema::tool::{
    CommandBody, CommandIndex, CommandNode, CommandTree, Tool, ToolMiddleware, ToolMiddlewareScope,
};
use golem_common::schema::{IntoTypedSchemaValue, SchemaGraph, SchemaType};
use golem_common::serialization::{deserialize, serialize};
use std::fmt::Debug;
use std::io::Write;
use std::path::Path;
use test_r::test;
use uuid::Uuid;

fn standalone_tool_activation() -> ToolActivationSnapshot {
    let definition = Tool {
        version: "1.2.0".to_string(),
        requires_filesystem: false,
        http_mount: None,
        commands: CommandTree {
            nodes: ["report", "daily", "send"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| CommandNode {
                    name: name.to_string(),
                    aliases: vec![],
                    doc: Default::default(),
                    http: None,
                    globals: Default::default(),
                    subcommands: if index < 2 {
                        vec![CommandIndex(index as i32 + 1)]
                    } else {
                        vec![]
                    },
                    body: (index == 2).then(|| CommandBody {
                        positionals: Default::default(),
                        options: vec![],
                        flags: vec![],
                        constraints: vec![],
                        stdin: None,
                        stdout: None,
                        stderr: None,
                        result: None,
                        errors: vec![],
                        annotations: None,
                    }),
                })
                .collect(),
        },
        schema: SchemaGraph::empty(),
    };
    let source = ToolSource::Component {
        component_id: ComponentId(Uuid::from_u128(1)),
        component_revision: 5u64.try_into().unwrap(),
        component_name: "test:report".try_into().unwrap(),
    };
    let plugin = |grant, destination: &str| InstalledPlugin {
        environment_plugin_grant_id: Uuid::from_u128(grant).into(),
        priority: PluginPriority(0),
        parameters: [("destination".to_string(), destination.to_string())].into(),
        plugin_registration_id: Uuid::from_u128(20).into(),
        plugin_name: "audit-processor".to_string(),
        plugin_version: "1.0.0".to_string(),
        oplog_processor_component_id: Some(ComponentId(Uuid::from_u128(21))),
        oplog_processor_component_revision: Some(2u64.try_into().unwrap()),
        installed_by: AccountId(Uuid::from_u128(4)),
    };
    let binding = CompiledToolBinding {
        deployment_revision: 8u64.try_into().unwrap(),
        release_id: Some(Uuid::from_u128(22).into()),
        owner: ToolBindingOwner::ComponentBaseline {
            component_id: ComponentId(Uuid::from_u128(1)),
        },
        tool_name: "report".try_into().unwrap(),
        version: definition.version.clone(),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
        account_id: AccountId(Uuid::from_u128(4)),
        account_email: "report-owner@example.com".into(),
        parameters: NormalizedJsonValue::new(serde_json::json!({"region": "eu"})),
        config_keys_readable: Default::default(),
        secret_keys_readable: Default::default(),
        secret_keys_revealable: Default::default(),
        filesystem_access: ToolFilesystemAccess::Denied,
        source: source.clone(),
        shell: false,
    };
    let middleware_chain = CompiledToolMiddlewareChain {
        deployment_revision: binding.deployment_revision,
        owner: binding.owner.clone(),
        tool_name: binding.tool_name.clone(),
        effective_definition: definition.clone(),
        occurrences: [("audit", plugin(32, "c")), ("metrics", plugin(33, "d"))]
            .into_iter()
            .enumerate()
            .map(|(index, (name, processor))| {
                let provision = ToolProvisionConfig {
                    config: NormalizedJsonValue::new(serde_json::json!({"enabled": true})),
                    env: [("PIPELINE_STAGE".to_string(), name.to_string())].into(),
                    plugins: vec![processor],
                    files: vec![],
                };
                CompiledToolMiddlewareOccurrence {
                    middleware: RegisteredToolMiddleware {
                        deployment_revision: binding.deployment_revision,
                        release_id: Some(Uuid::from_u128(40 + index as u128).into()),
                        definition: ToolMiddleware {
                            name: name.to_string(),
                            version: "2.0.0".to_string(),
                            aliases: vec![],
                            doc: Default::default(),
                            scope: ToolMiddlewareScope::Universal,
                            parameter_schema: SchemaGraph::anonymous(SchemaType::string()),
                        },
                        provision: provision.clone(),
                        source: ToolMiddlewareSource::Component {
                            component_id: ComponentId(Uuid::from_u128(50 + index as u128)),
                            component_revision: 7u64.try_into().unwrap(),
                            component_name: format!("test:{name}").try_into().unwrap(),
                        },
                        owner_account_id: binding.account_id,
                        owner_account_email: binding.account_email.clone(),
                        metadata_version: "0.1.0".to_string(),
                        metadata_digest: Default::default(),
                    },
                    parameters: name.to_string().into_typed_schema_value().unwrap(),
                    provision,
                    config_keys_readable: Default::default(),
                    secret_keys_readable: Default::default(),
                    secret_keys_revealable: Default::default(),
                    filesystem_access: ToolFilesystemAccess::Denied,
                    expected_definition: Some(definition.clone()),
                    presented_definition: Some(definition.clone()),
                    next_effective_definition: definition.clone(),
                    compatibility: None,
                }
            })
            .collect(),
    };
    ToolActivationSnapshot {
        registered_tool: ToolRegistration {
            deployment_revision: binding.deployment_revision,
            release_id: binding.release_id,
            definition,
            provision: ToolProvisionConfig {
                config: NormalizedJsonValue::new(serde_json::json!({"format": "json"})),
                env: [("REPORT_BUCKET".to_string(), "daily-reports".to_string())].into(),
                plugins: vec![plugin(30, "a"), plugin(31, "b")],
                files: vec![],
            },
            component_bindings: Default::default(),
            source,
            owner_account_id: binding.account_id,
            owner_account_email: binding.account_email.clone(),
            metadata_version: binding.metadata_version.clone(),
            metadata_digest: binding.metadata_digest,
        },
        binding,
        filesystem: FilesystemCapability::Incapable,
        middleware_chain: Some(middleware_chain),
        mcp_import: None,
    }
}

#[test]
fn standalone_tool_create_roundtrips() {
    let invocation_id = InvocationId(Uuid::from_u128(13));
    let activation = standalone_tool_activation();
    let metadata = CreateParameters {
        component_id: ComponentId(Uuid::from_u128(1)),
        agent_type_name: None,
        canonical_key: None,
        constructor_parameters: None,
        agent_id: AgentId::from_parts(7, 3),
        owner_kind: OwnerKind::StandaloneTool,
        agent_mode: AgentMode::Durable,
        component_revision: ComponentRevision::try_from(5u64).unwrap(),
        env: vec![],
        environment_id: EnvironmentId(Uuid::from_u128(2)),
        created_by: AccountId(Uuid::from_u128(4)),
        parent: None,
        component_size: 123,
        initial_total_linear_memory_size: 456,
        initial_active_plugins: Default::default(),
        local_agent_config: vec![],
        instance_id: Uuid::from_u128(6),
        create_request: CreateRequest {
            kind: CreateRequestKind::GetOrCreate,
            idempotency_key: Some(IdempotencyKey::new(invocation_id.to_string())),
            caller: Caller::anonymous(),
            trace_id: TraceId::from_string("00112233445566778899aabbccddeeff").unwrap(),
            trace_states: vec![],
            invocation_context: vec![],
        },
        standalone_tool: Some(Box::new(StandaloneToolInvocation {
            tool_name: "report".to_string().try_into().unwrap(),
            command_path: vec!["daily".to_string(), "send".to_string()],
            invocation_id,
            caller: Caller::anonymous(),
            activation,
        })),
        never_live: false,
    };
    let entry = OplogEntry::Create {
        timestamp: Timestamp::from(1_700_000_000_000u64),
        parameters: Box::new(metadata),
    };
    let encoded = serialize(&entry).unwrap();
    let decoded = deserialize::<OplogEntry>(&encoded).unwrap();
    assert_eq!(decoded, entry);
    let OplogEntry::Create { parameters, .. } = &decoded else {
        panic!("expected Create");
    };
    let pinned = &parameters.standalone_tool.as_ref().unwrap().activation;
    assert_eq!(
        pinned
            .processor_plugins()
            .map(|plugin| plugin.parameters["destination"].as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c", "d"]
    );
    let proto: RawOplogEntry = entry.clone().try_into().unwrap();
    assert_eq!(OplogEntry::try_from(proto).unwrap(), entry);
    let mut mint = Mint::new("tests/goldenfiles");
    mint.new_goldenfile("standalone_tool_create.bin")
        .unwrap()
        .write_all(&encoded)
        .unwrap();
}

#[allow(unused)]
fn is_deserializable<T: BinaryCodec + PartialEq + Debug>(old: &Path, new: &Path) {
    let old = std::fs::read(old).unwrap();
    let new = std::fs::read(new).unwrap();

    // Both the old and the latest binary can be deserialized
    let old_decoded: T = deserialize(&old).unwrap();
    let new_decoded: T = deserialize(&new).unwrap();

    // And they represent the same value
    assert_eq!(old_decoded, new_decoded);
}

#[allow(unused)]
pub(crate) fn backward_compatible_custom<T: BinaryCodec + Debug + 'static>(
    name: impl AsRef<str>,
    mint: &mut Mint,
    value: T,
    differ: Differ,
) {
    let mut file = mint
        .new_goldenfile_with_differ(format!("{}.bin", name.as_ref()), differ)
        .unwrap();
    let encoded = serialize(&value).unwrap();
    file.write_all(&encoded).unwrap();
    file.flush().unwrap();
}

#[allow(unused)]
pub(crate) fn backward_compatible<T: BinaryCodec + PartialEq + Debug + 'static>(
    name: impl AsRef<str>,
    mint: &mut Mint,
    value: T,
) {
    backward_compatible_custom(name, mint, value, Box::new(is_deserializable::<T>))
}

#[test]
fn stream_terminal_oplog_roundtrips() {
    use golem_common::model::Timestamp;
    use golem_common::model::durable_stream::{
        DURABLE_STREAM_FORMAT_VERSION, LocalStreamId, StreamError, StreamOffset, StreamTerminal,
        StreamTerminalAuthor, StreamTerminalOrigin, StreamTerminalRecord,
    };
    use golem_common::model::oplog::payload::OplogPayload;
    use golem_common::model::oplog::{OplogEntry, OplogIndex};
    use golem_common::schema::IntoTypedSchemaValue;

    let declared = (73u32, "producer rejected input".to_string())
        .into_typed_schema_value()
        .unwrap();
    let cases = [
        StreamTerminal::Finished,
        StreamTerminal::Failed(StreamError::Failed(Box::new(declared))),
        StreamTerminal::Failed(StreamError::Trapped("producer trapped".into())),
        StreamTerminal::Failed(StreamError::Cancelled),
        StreamTerminal::Failed(StreamError::Abandoned),
        StreamTerminal::Failed(StreamError::Interrupted("source unavailable".into())),
        StreamTerminal::Failed(StreamError::ResourceExhausted("retained bytes".into())),
        StreamTerminal::ConsumerCancelled {
            details: Some("reader dropped".into()),
        },
        StreamTerminal::ConsumerCancelled { details: None },
    ];
    for authored_by in [
        StreamTerminalAuthor::Guest,
        StreamTerminalAuthor::Host,
        StreamTerminalAuthor::Protocol,
    ] {
        for terminal in &cases {
            for origin in [StreamTerminalOrigin::Source, StreamTerminalOrigin::Control] {
                let record = StreamTerminalRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id: LocalStreamId(OplogIndex::from_u64(19)),
                    sequence: 7,
                    offset: StreamOffset::new(OplogIndex::from_u64(31), 0),
                    authored_by,
                    origin,
                    terminal: terminal.clone(),
                };
                let record_bytes = serialize(&record).unwrap();
                assert_eq!(
                    deserialize::<StreamTerminalRecord>(&record_bytes).unwrap(),
                    record
                );
                let entry = OplogEntry::StreamTerminal {
                    timestamp: Timestamp::from(1_700_000_000_000u64),
                    entity_parent_start_index: Some(OplogIndex::from_u64(11)),
                    summary: None,
                    record: OplogPayload::SerializedInline {
                        bytes: record_bytes,
                        cached: None,
                    },
                };
                let encoded = serialize(&entry).unwrap();
                assert_eq!(deserialize::<OplogEntry>(&encoded).unwrap(), entry);
            }
        }
    }
}
