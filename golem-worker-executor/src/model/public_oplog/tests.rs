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

use super::*;
use crate::services::oplog::{CommitLevel, OplogOps, PrimaryOplogService};
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentPrincipal, AgentTypeName, Principal};
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::entity::{
    EntityActivation, EntityActivationPolicy, EntityInvocationPlan, EntityInvocationPlanLayer,
    EntityInvocationPlanReference, ExecutableTarget, FilesystemCapability,
    ToolInvocationDescriptor, ToolMiddlewareName,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::oplog::payload::host_functions::HostFunctionName;
use golem_common::model::oplog::payload::types::{
    SecretRevealAudit, SerializableDateTime, SerializableEntityBodyExecution,
    SerializableHttpErrorCode, SerializableHttpMethod, SerializableIpAddress,
    SerializableP3HttpBodyChunk, SerializableP3HttpClientSend, SerializableP3HttpClientSendResult,
    SerializableP3HttpConsumeBodyResult, SerializableP3HttpRequestOptions,
    SerializableP3HttpScheme, SerializableP3IpSocketAddress, SerializableP3SocketErrorCode,
    SerializableP3TcpChunk, SerializableP3UdpDatagram, SerializableResponseHeaders,
    SerializableToolOperationTerminal, SerializableToolRpcError, SerializableToolStructuredResult,
};
use golem_common::model::oplog::{
    AttributeMap, DurableFunctionType, HostRequestEntityInvocation,
    HostRequestGolemToolInvocationRejected, HostRequestNoInput, HostRequestP3HttpClientSend,
    HostRequestP3SocketsUdpSend, HostRequestSecretReveal, HostResponseEntityInvocation,
    HostResponseP3BlobstoreIncomingValueStream, HostResponseP3HttpClientConsumeBodyChunk,
    HostResponseP3HttpClientConsumeBodyResult, HostResponseP3HttpClientSendResult,
    HostResponseP3KeyvalueIncomingValueStream, HostResponseP3SocketsTcpAcquire,
    HostResponseP3SocketsTcpReceiveChunk, HostResponseP3SocketsUdpReceive,
    HostResponseP3SocketsUdpSend, HostResponseSecretRevealed, HostStreamKind, LogLevel,
    OplogPayload,
};
use golem_common::model::tool::{
    CompiledToolBinding, SecretKeyScope, ToolFilesystemAccess, ToolName, ToolProvisionConfig,
    ToolSource,
};
use golem_common::model::{
    AgentFingerprint, AgentMetadata, AgentStatusRecord, RetryConfig, Timestamp, TransactionId,
};
use golem_common::read_only_lock;
use golem_common::schema::tool::{CommandTree, Tool};
use golem_common::schema::{
    IntoTypedSchemaValue, SchemaGraph, SchemaType, SchemaValue, SecretValuePayload,
};
use golem_service_base::model::component::Component;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use prost::Message;
use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;
use test_r::test;
use uuid::Uuid;

#[test]
fn tool_attachment_read_contract_roundtrips() {
    use crate::preview2::golem_api_1_x::oplog;
    use golem_common::base_model::durable_stream::StreamConsumerObservation;
    use golem_common::base_model::oplog::attachment::{
        AcceptedAttachmentWriteEnd, ToolAttachmentId, ToolAttachmentReadObservation,
        ToolAttachmentReadOutcome, ToolAttachmentSlot,
    };
    use golem_common::model::{IdempotencyKey, OplogIndex};
    for (outcome, reobserves, payload) in [
        (
            ToolAttachmentReadOutcome::RawBytes {
                end: AcceptedAttachmentWriteEnd(OplogIndex::from_u64(7)),
                offset: 2,
                length: 3,
            },
            None,
            Some(vec![1, 2, 3]),
        ),
        (
            ToolAttachmentReadOutcome::Eof {
                terminal_index: OplogIndex::from_u64(8),
            },
            None,
            None,
        ),
        (
            ToolAttachmentReadOutcome::TypedChunk(AcceptedAttachmentWriteEnd(
                OplogIndex::from_u64(7),
            )),
            Some(OplogIndex::from_u64(9)),
            None,
        ),
    ] {
        let observation = ToolAttachmentReadObservation {
            attachment: ToolAttachmentId {
                creation_start: OplogIndex::from_u64(3),
                slot: ToolAttachmentSlot::Stdout,
            },
            logical_invocation: IdempotencyKey::fresh(),
            observation: StreamConsumerObservation::Guest {
                parent_start_index: Some(OplogIndex::from_u64(8)),
            },
            ordinal: 1,
            read_attempt_ordinal: 4,
            outcome,
            reobserves,
        };
        let raw = OplogEntry::ToolAttachmentRead {
            timestamp: Timestamp::now_utc().rounded(),
            entity_parent_start_index: Some(OplogIndex::from_u64(2)),
            observation: Box::new(observation.clone()),
            payload: payload
                .as_ref()
                .map(|bytes| OplogPayload::SerializedInline {
                    bytes: golem_common::serialization::serialize(bytes).unwrap(),
                    cached: None,
                }),
        };
        let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
            raw.clone().try_into().unwrap();
        let restored = OplogEntry::try_from(proto).unwrap();
        assert_eq!(restored, raw);
        let wire: oplog::OplogEntry = restored.try_into().unwrap();
        assert_eq!(OplogEntry::try_from(wire).unwrap(), raw);
        let mut wrong_ownership = raw.clone();
        let OplogEntry::ToolAttachmentRead {
            payload: wrong_payload,
            ..
        } = &mut wrong_ownership
        else {
            unreachable!()
        };
        *wrong_payload = if wrong_payload.is_some() {
            None
        } else {
            Some(OplogPayload::Inline(Box::new(vec![42])))
        };
        let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
            wrong_ownership.clone().try_into().unwrap();
        assert!(OplogEntry::try_from(proto).is_err());
        let wire: oplog::OplogEntry = wrong_ownership.try_into().unwrap();
        assert!(OplogEntry::try_from(wire).is_err());
        if payload.is_some() {
            let mut external = raw.clone();
            let OplogEntry::ToolAttachmentRead { payload, .. } = &mut external else {
                unreachable!()
            };
            *payload = Some(OplogPayload::External {
                payload_id: golem_common::model::oplog::PayloadId::new(),
                md5_hash: vec![3; 16],
                cached: None,
            });
            let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
                external.clone().try_into().unwrap();
            let restored = OplogEntry::try_from(proto).unwrap();
            let wire: oplog::OplogEntry = restored.try_into().unwrap();
            assert_eq!(OplogEntry::try_from(wire).unwrap(), external);
        }

        let public = PublicOplogEntry::ToolAttachmentRead(
            golem_common::model::oplog::public_oplog_entry::ToolAttachmentReadParams {
                timestamp: raw.timestamp(),
                observation: observation.into_typed_schema_value().unwrap(),
                payload,
            },
        );
        let proto: golem_api_grpc::proto::golem::worker::OplogEntry =
            public.clone().try_into().unwrap();
        assert_eq!(PublicOplogEntry::try_from(proto).unwrap(), public);
        let mut wrong_ownership = public.clone();
        let PublicOplogEntry::ToolAttachmentRead(params) = &mut wrong_ownership else {
            unreachable!()
        };
        params.payload = if params.payload.is_some() {
            None
        } else {
            Some(vec![42])
        };
        let proto: golem_api_grpc::proto::golem::worker::OplogEntry =
            wrong_ownership.clone().try_into().unwrap();
        assert!(PublicOplogEntry::try_from(proto).is_err());
        assert!(OplogEntry::try_from(wrong_ownership).is_err());
        let public_raw = OplogEntry::try_from(public.clone()).unwrap();
        let OplogEntry::ToolAttachmentRead {
            observation: restored_observation,
            payload: restored_payload,
            entity_parent_start_index,
            ..
        } = public_raw
        else {
            panic!("wrong raw variant")
        };
        assert_eq!(*restored_observation, observation);
        assert_eq!(
            restored_payload
                .map(|payload| golem_common::serialization::serialize(&payload).unwrap()),
            raw_payload(&raw)
        );
        assert_eq!(entity_parent_start_index, None);
        let wire: oplog::PublicOplogEntry = public.clone().try_into().unwrap();
        let oplog::PublicOplogEntry::ToolAttachmentRead(wire) = wire else {
            panic!("wrong public variant")
        };
        let PublicOplogEntry::ToolAttachmentRead(params) = public else {
            unreachable!()
        };
        assert_eq!(
            golem_schema::schema::wit::decode_typed(&wire.observation).unwrap(),
            params.observation
        );
        assert_eq!(wire.payload, params.payload);
    }

    fn raw_payload(entry: &OplogEntry) -> Option<Vec<u8>> {
        match entry {
            OplogEntry::ToolAttachmentRead { payload, .. } => payload
                .as_ref()
                .map(|payload| golem_common::serialization::serialize(payload).unwrap()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn tool_attachment_read_rejects_invalid_observation_schema() {
    use crate::preview2::golem_api_1_x::oplog;
    let invalid = "not an attachment observation"
        .into_typed_schema_value()
        .unwrap();
    let public = golem_api_grpc::proto::golem::worker::OplogEntry {
        entry: Some(
            golem_api_grpc::proto::golem::worker::oplog_entry::Entry::ToolAttachmentRead(
                golem_api_grpc::proto::golem::worker::ToolAttachmentReadParameters {
                    timestamp: Some(Timestamp::now_utc().into()),
                    observation: Some(invalid.clone().try_into().unwrap()),
                    payload: None,
                },
            ),
        ),
    };
    assert!(PublicOplogEntry::try_from(public).is_err());
    let raw_proto = golem_api_grpc::proto::golem::worker::RawOplogEntry {
        timestamp: Some(Timestamp::now_utc().into()),
        entity_parent_start_index: None,
        entry: Some(
            golem_api_grpc::proto::golem::worker::raw_oplog_entry::Entry::ToolAttachmentRead(
                golem_api_grpc::proto::golem::worker::RawToolAttachmentReadParameters {
                    observation: Some(invalid.clone().try_into().unwrap()),
                    payload: None,
                },
            ),
        ),
    };
    assert!(OplogEntry::try_from(raw_proto).is_err());
    let raw = oplog::OplogEntry::ToolAttachmentRead(oplog::RawToolAttachmentReadParameters {
        timestamp: Timestamp::now_utc().into(),
        entity_parent_start_index: None,
        observation: golem_schema::schema::wit::encode_typed(&invalid).unwrap(),
        payload: None,
    });
    assert!(OplogEntry::try_from(raw).is_err());
}

#[test]
fn raw_stream_telemetry_survives_uncached_payload_roundtrips() {
    use crate::preview2::golem_api_1_x::oplog;
    use golem_common::model::oplog::{
        DurableStreamEventSummary as Summary, DurableStreamOutcome as Outcome, PayloadId,
    };

    for external in [false, true] {
        macro_rules! entry {
            ($variant:ident, $summary:expr) => {
                OplogEntry::$variant {
                    timestamp: Timestamp::now_utc().rounded(),
                    entity_parent_start_index: None,
                    summary: $summary,
                    record: if external {
                        OplogPayload::External {
                            payload_id: PayloadId::new(),
                            md5_hash: vec![3; 16],
                            cached: None,
                        }
                    } else {
                        OplogPayload::SerializedInline {
                            bytes: vec![0xff, 0x01],
                            cached: None,
                        }
                    },
                }
            };
        }
        let entries = [
            entry!(StreamRegistered, Some(Summary::Registered)),
            entry!(StreamItems, Some(Summary::Items { item_count: 3 })),
            entry!(
                StreamEnd,
                Some(Summary::End {
                    outcome: Outcome::Success
                })
            ),
            entry!(
                StreamEnd,
                Some(Summary::End {
                    outcome: Outcome::Error
                })
            ),
            entry!(StreamCancel, Some(Summary::Cancelled)),
            entry!(StreamSession, Some(Summary::SessionResult)),
            entry!(
                StreamSession,
                Some(Summary::SessionFinished {
                    outcome: Outcome::Error
                })
            ),
            entry!(
                StreamSession,
                Some(Summary::SessionFinished {
                    outcome: Outcome::Success
                })
            ),
            entry!(StreamSession, Some(Summary::SessionCancellation)),
            entry!(StreamSession, Some(Summary::SessionExpired)),
            entry!(StreamSession, None),
        ];
        for entry in entries {
            let bytes = golem_common::serialization::serialize(&entry).unwrap();
            let stored: OplogEntry = golem_common::serialization::deserialize(&bytes).unwrap();
            let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
                stored.try_into().unwrap();
            let stored = OplogEntry::try_from(proto).unwrap();
            let wire: oplog::OplogEntry = stored.try_into().unwrap();
            let restored = OplogEntry::try_from(wire).unwrap();
            assert_eq!(restored, entry);
        }
    }
}

#[test]
fn raw_log_trace_context_survives_wit_roundtrip() {
    use crate::preview2::golem_api_1_x::oplog;
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::oplog::LogTraceContext;

    for trace_context in [
        None,
        Some(LogTraceContext {
            trace_id: TraceId::from_string("00112233445566778899aabbccddeeff").unwrap(),
            span_id: SpanId::from_string("0123456789abcdef").unwrap(),
        }),
    ] {
        let entry = OplogEntry::Log {
            timestamp: Timestamp::now_utc().rounded(),
            parent_start_index: None,
            level: LogLevel::Stdout,
            context: "context".to_string(),
            message: "buffered output".to_string(),
            trace_context,
        };
        let wire: oplog::OplogEntry = entry.clone().try_into().unwrap();
        assert_eq!(OplogEntry::try_from(wire).unwrap(), entry);
    }
}

/// Component service stub for entries whose rendering must not need component
/// metadata (`Start`/`End`/`Cancelled` host call entries).
struct PanicComponentService;

fn test_tool_definition() -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        commands: CommandTree { nodes: Vec::new() },
        schema: SchemaGraph::empty(),
    }
}

#[async_trait]
impl ComponentService for PanicComponentService {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: golem_common::model::component::ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        panic!("component service must not be used when rendering host call entries")
    }

    async fn get_metadata(
        &self,
        _component_id: golem_common::model::component::ComponentId,
        _forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        panic!("component service must not be used when rendering host call entries")
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: golem_common::model::application::ApplicationId,
        _resolving_account: golem_common::model::account::AccountId,
    ) -> Result<Option<golem_common::model::component::ComponentId>, WorkerExecutorError> {
        panic!("component service must not be used when rendering host call entries")
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        Vec::new()
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
}

fn make_agent_metadata(
    agent_id: AgentId,
    created_by: AccountId,
    environment_id: EnvironmentId,
) -> AgentMetadata {
    AgentMetadata {
        agent_id,
        owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
        env: vec![],
        environment_id,
        created_by,
        created_by_email: AccountEmail::new("test@golem"),
        config: Vec::new(),
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord::default(),
        original_phantom_id: None,
        fingerprint: AgentFingerprint::new(),
        agent_mode: AgentMode::Durable,
    }
}

fn default_last_known_status() -> read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord> {
    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(arc_swap::ArcSwap::from_pointee(
        AgentStatusRecord::default(),
    )))
}

fn default_execution_status(
    agent_mode: AgentMode,
) -> read_only_lock::std::ReadOnlyLock<crate::model::ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
        crate::model::ExecutionStatus::Suspended {
            agent_mode,
            timestamp: Timestamp::now_utc(),
        },
    )))
}

fn header_map(key: &str, value: &[u8]) -> HashMap<String, Vec<Vec<u8>>> {
    HashMap::from_iter(vec![(key.to_string(), vec![value.to_vec()])])
}

fn test_entity_activation(entity: &AgentEntity) -> EntityActivation {
    let component_id = golem_common::model::component::ComponentId::new();
    let component_revision = ComponentRevision::new(1).unwrap();
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let executable = ExecutableTarget::new(component_id, component_revision);
    let policy = match entity {
        AgentEntity::Tool(tool_name) => EntityActivationPolicy::Tool {
            provision: ToolProvisionConfig::default(),
            mcp_import: None,
            binding: Box::new(CompiledToolBinding {
                deployment_revision,
                release_id: None,
                owner: golem_common::model::tool::ToolBindingOwner::AgentType {
                    agent_type_name: AgentTypeName("Agent".to_string()),
                },
                tool_name: tool_name.clone(),
                version: "1".to_string(),
                metadata_version: "1".to_string(),
                metadata_digest: Default::default(),
                account_id: AccountId::new(),
                account_email: AccountEmail::new("owner@example.com"),
                parameters: NormalizedJsonValue::new(serde_json::json!({})),
                config_keys_readable: Default::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
                source: ToolSource::Component {
                    component_id,
                    component_revision,
                    component_name: ComponentName("tools".to_string()),
                },
            }),
        },
        AgentEntity::ToolMiddleware(middleware_name) => EntityActivationPolicy::ToolMiddleware {
            middleware_name: middleware_name.clone(),
            provision: ToolProvisionConfig::default(),
            config_keys_readable: Default::default(),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Unset,
        },
    };
    EntityActivation::new(
        executable,
        deployment_revision,
        policy,
        FilesystemCapability::Incapable,
    )
    .unwrap()
}

fn test_entity_request(
    owner: &OwnedAgentId,
    entity: AgentEntity,
    call_mode: EntityCallMode,
    operation: Option<EntityInvocationDescriptor>,
    input: TypedSchemaValue,
) -> HostRequest {
    let activation = test_entity_activation(&entity);
    let plan = match &entity {
        AgentEntity::Tool(_) => {
            EntityInvocationPlan::new(vec![EntityInvocationPlanLayer::Tool { activation }])
        }
        AgentEntity::ToolMiddleware(_) => EntityInvocationPlan::new(vec![
            EntityInvocationPlanLayer::Middleware {
                activation,
                parameters: TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
                    SchemaValue::Tuple {
                        elements: Vec::new(),
                    },
                ),
                expected_definition: None,
                presented_definition: None,
                next_effective_definition: test_tool_definition(),
                compatibility: None,
            },
            EntityInvocationPlanLayer::Tool {
                activation: test_entity_activation(&AgentEntity::Tool(
                    ToolName::try_from("test").unwrap(),
                )),
            },
        ]),
    }
    .unwrap();
    let metadata = EntityInvocationRequest {
        entity,
        call_mode,
        operation: operation.unwrap_or_else(|| {
            EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
                attempt_ordinal: 0,
                command_path: vec!["test".to_string()],
                args: Vec::new(),
                has_stdin: false,
                has_stdout: false,
                declares_stdout: false,
                has_stderr: false,
                declares_stderr: false,
                output_contract: golem_common::model::entity::ToolOutputContract {
                    result: None,
                    errors: Vec::new(),
                },
            })
        }),
        principal: Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        }),
        plan: EntityInvocationPlanReference::Root { plan },
        authority_wallet: Vec::new(),
    };
    HostRequestEntityInvocation {
        metadata: desert_rust::serialize_to_byte_vec(&metadata).unwrap(),
        input,
        stream_session_idempotency_key: golem_common::model::IdempotencyKey::new(
            "entity-stream-session".to_string(),
        ),
    }
    .into()
}

#[test]
async fn public_oplog_zero_start_reads_from_initial_index() {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            blob_storage,
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId(Uuid::new_v4()),
        agent_id: "public-oplog-zero-start".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();
    let timestamp = Timestamp::now_utc();
    assert_eq!(
        oplog
            .add(OplogEntry::RecoverySucceeded { timestamp })
            .await
            .unwrap(),
        OplogIndex::INITIAL
    );
    oplog.commit(CommitLevel::Always).await.unwrap();

    let chunk = get_public_oplog_chunk(
        Arc::new(PanicComponentService),
        oplog_service,
        &owned_agent_id,
        AgentMode::Durable,
        None,
        ComponentRevision::INITIAL,
        OplogIndex::NONE,
        1,
    )
    .await
    .unwrap();

    assert_eq!(chunk.first_index_in_chunk, OplogIndex::INITIAL);
    assert_eq!(chunk.next_oplog_index, OplogIndex::from_u64(2));
    assert_eq!(chunk.entries.len(), 1);
    assert!(matches!(
        chunk.entries[0].entry,
        PublicOplogEntry::RecoverySucceeded(_)
    ));
}

#[test]
async fn entity_attribution_is_nested_page_independent_and_order_preserving() {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            blob_storage,
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId::new(),
        agent_id: "Agent(\"entity-attribution\")".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();

    let agent_entry = oplog
        .add(OplogEntry::remove_retry_policy(
            None,
            "test-policy".to_string(),
        ))
        .await
        .unwrap();
    let observational_owner = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("agent-custom-owner".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    let middleware_entity =
        AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("audit").unwrap());
    let middleware_input = "middleware-input"
        .to_string()
        .into_typed_schema_value()
        .unwrap();
    let middleware_request = test_entity_request(
        &owned_agent_id,
        middleware_entity,
        EntityCallMode::Synchronous,
        None,
        middleware_input.clone(),
    );
    let middleware_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::GolemEntityInvoke,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(middleware_request))),
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();

    let interleaved_agent_entry = oplog
        .add(OplogEntry::remove_retry_policy(
            None,
            "test-policy".to_string(),
        ))
        .await
        .unwrap();
    let child_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(middleware_start),
            function_name: HostFunctionName::Custom("entity-child".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequestNoInput {}.into()))),
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();

    let tool_entity = AgentEntity::Tool(ToolName::try_from("lookup").unwrap());
    let secret_id = Uuid::from_u128(1);
    let tool_input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::secret(
            golem_common::schema::schema_type::SecretSpec::default(),
        )),
        SchemaValue::Secret(SecretValuePayload {
            secret_id,
            config_key: Some(vec!["database".to_string(), "password".to_string()]),
            version: 7,
            resolved_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            category: Some("api-key".to_string()),
        }),
    );
    let tool_request = test_entity_request(
        &owned_agent_id,
        tool_entity,
        EntityCallMode::Asynchronous,
        Some(EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 0,
            command_path: vec!["files".to_string(), "lookup".to_string()],
            args: vec![golem_common::model::card::ToolArgPattern::Positional(
                golem_common::model::card::ToolInvocationValue::Literal(
                    "configured-secret-rendering".into(),
                ),
            )],
            has_stdin: true,
            has_stdout: true,
            declares_stdout: true,
            has_stderr: true,
            declares_stderr: true,
            output_contract: golem_common::model::entity::ToolOutputContract {
                result: None,
                errors: Vec::new(),
            },
        })),
        tool_input.clone(),
    );
    let tool_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(child_start),
            function_name: HostFunctionName::GolemEntityInvoke,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(tool_request))),
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: Some(Box::new(SpanStarted {
                span_id: SpanId::generate(),
                trace_id: golem_common::model::invocation_context::TraceId::generate(),
                trace_states: Vec::new(),
                parent_span_id: None,
                links: Vec::new(),
                started_at: Timestamp::now_utc(),
                attributes: AttributeMap(HashMap::new()),
                kind: SpanKind::Internal,
            })),
            denial: None,
        })
        .await
        .unwrap();
    let entity_retry_error = oplog
        .add(OplogEntry::error(
            Some(tool_start),
            golem_common::model::oplog::OplogErrorKind::Invocation,
            golem_common::model::oplog::AgentError::TransientError("entity retry".to_string()),
            agent_entry,
            None,
        ))
        .await
        .unwrap();
    let entity_marker = oplog
        .add(OplogEntry::remove_retry_policy(
            Some(tool_start),
            "test-policy".to_string(),
        ))
        .await
        .unwrap();
    let log_index = oplog
        .add(OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            level: LogLevel::Info,
            context: "tool".to_string(),
            message: "entity-attribution-needle".to_string(),
            trace_context: None,
        })
        .await
        .unwrap();
    let stream_frame_index = oplog
        .add(OplogEntry::HostStreamFrame {
            timestamp: Timestamp::now_utc(),
            parent_start_index: tool_start,
            kind: HostStreamKind::P3HttpRequestBody,
            payload: OplogPayload::Inline(Box::new(HostRequestNoInput {}.into())),
        })
        .await
        .unwrap();

    let reveal_secret_id = Uuid::from_u128(2);
    let reveal_request = HostRequestSecretReveal {
        secret_id: reveal_secret_id,
        expected_type: SchemaGraph::anonymous(SchemaType::string()),
    };
    let reveal_response = HostResponseSecretRevealed {
        secret_id: reveal_secret_id,
        pinned_revision: 9,
        resolved_at: SerializableDateTime {
            seconds: 1_700_000_002,
            nanoseconds: 0,
        },
        result: Ok(()),
        audit: SecretRevealAudit {
            calling_agent: AgentId {
                component_id: golem_common::model::component::ComponentId(Uuid::nil()),
                agent_id: "secret-reveal-auditor".to_string(),
            },
            config_key: Some(vec!["database".to_string(), "password".to_string()]),
            timestamp: SerializableDateTime {
                seconds: 1_700_000_003,
                nanoseconds: 0,
            },
        },
    };
    let reveal_request_payload: HostRequest = reveal_request.clone().into();
    let reveal_response_payload: HostResponse = reveal_response.clone().into();
    let (reveal_start, reveal_end) = oplog
        .add_completed_host_call(
            HostFunctionName::GolemSecretsReveal,
            &reveal_request_payload,
            &reveal_response_payload,
            DurableFunctionType::ReadRemote,
            Some(tool_start),
            None,
        )
        .await
        .unwrap();

    let observational_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            function_name: HostFunctionName::Custom("observational-call".to_string()),
            invocation_id: None,
            observational_owner: Some(observational_owner),
            request: None,
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    let observational_log = oplog
        .add(OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(observational_start),
            level: LogLevel::Info,
            context: "custom".to_string(),
            message: "agent-owned observation".to_string(),
            trace_context: None,
        })
        .await
        .unwrap();
    let observational_end = oplog
        .add(OplogEntry::end(
            observational_start,
            None,
            false,
            None,
            None,
        ))
        .await
        .unwrap();

    let transaction_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            function_name: HostFunctionName::Custom("transaction".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    let transaction_begin = oplog
        .add(OplogEntry::BeginRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            transaction_id: TransactionId::new("entity-transaction".to_string()),
            original_begin_index: transaction_start,
        })
        .await
        .unwrap();
    let transaction_commit = oplog
        .add(OplogEntry::CommittedRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index: transaction_start,
        })
        .await
        .unwrap();
    let transaction_end = oplog
        .add(OplogEntry::end(transaction_start, None, false, None, None))
        .await
        .unwrap();
    let child_end = oplog
        .add(OplogEntry::end(child_start, None, false, None, None))
        .await
        .unwrap();
    let tool_terminal = SerializableToolOperationTerminal {
        body_execution: SerializableEntityBodyExecution::Executed,
        result: Ok(SerializableToolStructuredResult { result: None }),
        admission_refusal: None,
    }
    .into_typed_schema_value()
    .unwrap();
    let tool_response: HostResponse = HostResponseEntityInvocation {
        result: Ok(tool_terminal.clone()),
    }
    .into();
    let tool_end = oplog
        .add(OplogEntry::end(
            tool_start,
            Some(OplogPayload::Inline(Box::new(tool_response))),
            false,
            None,
            None,
        ))
        .await
        .unwrap();
    let completion = oplog
        .add(OplogEntry::completion_delivered(tool_start))
        .await
        .unwrap();

    let rejected_request: HostRequest = HostRequestGolemToolInvocationRejected {
        attempt_ordinal: 0,
        tool_name: "rejected".to_string(),
        command_path: vec!["reject".to_string()],
        input: None,
        input_decode_failure: None,
        has_stdin: false,
        has_stdout: false,
        has_stderr: false,
        call_mode: EntityCallMode::Synchronous,
        error: SerializableToolRpcError::Denied("not allowed".to_string()),
    }
    .into();
    let rejected_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            function_name: HostFunctionName::GolemToolInvocationRejected,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(rejected_request))),
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    let rejected_end = oplog
        .add(OplogEntry::end(rejected_start, None, false, None, None))
        .await
        .unwrap();
    let middleware_end = oplog
        .add(OplogEntry::end(middleware_start, None, false, None, None))
        .await
        .unwrap();
    let final_log = oplog
        .add(OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            level: LogLevel::Info,
            context: "tool".to_string(),
            message: "last-entity-attribution-needle".to_string(),
            trace_context: None,
        })
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let components: Arc<dyn ComponentService> = Arc::new(PanicComponentService);
    let chunk = get_public_oplog_chunk(
        components.clone(),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        None,
        ComponentRevision::INITIAL,
        OplogIndex::INITIAL,
        final_log.as_u64() as usize,
    )
    .await
    .unwrap();
    let expected_order = (agent_entry.as_u64()..=final_log.as_u64())
        .map(OplogIndex::from_u64)
        .collect::<Vec<_>>();
    assert_eq!(
        chunk
            .entries
            .iter()
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>(),
        expected_order
    );
    assert!(matches!(
        chunk.entries[agent_entry.as_u64() as usize - 1].attribution,
        PublicOplogEntryAttribution::Agent(_)
    ));
    assert!(matches!(
        chunk.entries[interleaved_agent_entry.as_u64() as usize - 1].attribution,
        PublicOplogEntryAttribution::Agent(_)
    ));
    for index in [
        observational_owner,
        observational_start,
        observational_log,
        observational_end,
    ] {
        assert!(matches!(
            chunk.entries[index.as_u64() as usize - 1].attribution,
            PublicOplogEntryAttribution::Agent(_)
        ));
    }

    let middleware = &chunk.entries[middleware_start.as_u64() as usize - 1];
    let PublicOplogEntryAttribution::Entity(middleware_context) = &middleware.attribution else {
        panic!("middleware Start must be entity-attributed");
    };
    assert_eq!(middleware_context.invocation.entity.name, "audit");
    assert_eq!(middleware_context.invocation.start_index, middleware_start);
    assert!(middleware_context.ancestors.is_empty());
    let PublicOplogEntry::Start(middleware_params) = &middleware.entry else {
        panic!("expected middleware Start");
    };
    assert_eq!(middleware_params.request.as_ref(), Some(&middleware_input));

    for index in [
        tool_start,
        entity_retry_error,
        entity_marker,
        log_index,
        stream_frame_index,
        reveal_start,
        reveal_end,
        transaction_start,
        transaction_begin,
        transaction_commit,
        transaction_end,
        tool_end,
        completion,
        final_log,
    ] {
        let entry = &chunk.entries[index.as_u64() as usize - 1];
        let PublicOplogEntryAttribution::Entity(context) = &entry.attribution else {
            panic!("entry {index} must be attributed to the nested tool");
        };
        assert_eq!(context.invocation.entity.name, "lookup");
        assert_eq!(context.invocation.start_index, tool_start);
        assert_eq!(context.ancestors.len(), 1);
        assert_eq!(context.ancestors[0].entity.name, "audit");
        assert_eq!(context.ancestors[0].start_index, middleware_start);
    }
    for index in [child_start, child_end, middleware_end] {
        let entry = &chunk.entries[index.as_u64() as usize - 1];
        let PublicOplogEntryAttribution::Entity(context) = &entry.attribution else {
            panic!("entry {index} must be attributed to the middleware");
        };
        assert_eq!(context.invocation.start_index, middleware_start);
        assert!(context.ancestors.is_empty());
    }
    for index in [rejected_start, rejected_end] {
        assert!(matches!(
            chunk.entries[index.as_u64() as usize - 1].attribution,
            PublicOplogEntryAttribution::Agent(_)
        ));
    }

    let tool = &chunk.entries[tool_start.as_u64() as usize - 1];
    let PublicOplogEntry::Start(tool_params) = &tool.entry else {
        panic!("expected tool Start");
    };
    assert_eq!(tool_params.request.as_ref(), Some(&tool_input));
    let tool_json = serde_json::to_string(tool).unwrap();
    assert!(tool_json.contains(&secret_id.to_string()));
    assert!(tool_json.contains("database"));
    assert!(tool_json.contains("password"));
    assert!(tool_json.contains("api-key"));
    assert!(tool_json.contains("2023-11-14T22:13:20Z"));
    assert!(!tool_json.contains("secretValue"));
    assert!(!tool_json.contains("configured-secret-rendering"));
    assert!(!tool_json.contains("owner@example.com"));
    assert!(!tool_json.contains("metadata"));

    let PublicOplogEntry::Start(reveal_start_params) =
        &chunk.entries[reveal_start.as_u64() as usize - 1].entry
    else {
        panic!("expected secret reveal Start");
    };
    assert_eq!(
        reveal_start_params.request.as_ref(),
        Some(
            &reveal_request
                .into_typed_schema_value()
                .expect("secret reveal request must be schema-encodable")
        )
    );
    let PublicOplogEntry::End(reveal_end_params) =
        &chunk.entries[reveal_end.as_u64() as usize - 1].entry
    else {
        panic!("expected secret reveal End");
    };
    assert_eq!(
        reveal_end_params.response.as_ref(),
        Some(
            &reveal_response
                .into_typed_schema_value()
                .expect("secret reveal response must be schema-encodable")
        )
    );
    let reveal_json = serde_json::to_string(&[
        &chunk.entries[reveal_start.as_u64() as usize - 1],
        &chunk.entries[reveal_end.as_u64() as usize - 1],
    ])
    .unwrap();
    for safe_metadata in [
        "secret-reveal-auditor".to_string(),
        "database".to_string(),
        "password".to_string(),
        "1700000002".to_string(),
        "1700000003".to_string(),
    ] {
        assert!(
            reveal_json.contains(&safe_metadata),
            "expected {safe_metadata:?} in {reveal_json}"
        );
    }
    assert!(!reveal_json.contains("secretValue"));

    let tool_terminal_entry = &chunk.entries[tool_end.as_u64() as usize - 1];
    let PublicOplogEntry::End(tool_terminal_params) = &tool_terminal_entry.entry else {
        panic!("expected tool End");
    };
    assert_eq!(tool_terminal_params.response.as_ref(), Some(&tool_terminal));
    assert!(
        !serde_json::to_string(tool_terminal_entry)
            .unwrap()
            .contains("stdout")
    );

    let page = get_public_oplog_chunk(
        components.clone(),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        None,
        ComponentRevision::INITIAL,
        log_index,
        1,
    )
    .await
    .unwrap();
    assert_eq!(page.entries.len(), 1);
    let PublicOplogEntryAttribution::Entity(page_context) = &page.entries[0].attribution else {
        panic!("page beginning inside an entity must resolve historical attribution");
    };
    assert_eq!(page_context.invocation.start_index, tool_start);
    assert_eq!(page_context.ancestors[0].start_index, middleware_start);

    let search = search_public_oplog(
        components,
        oplog_service,
        &owned_agent_id,
        AgentMode::Durable,
        None,
        ComponentRevision::INITIAL,
        OplogIndex::INITIAL,
        1,
        "last-entity-attribution-needle",
    )
    .await
    .unwrap();
    assert_eq!(search.entries.len(), 1);
    assert_eq!(search.entries[0].oplog_index, final_log);
    let PublicOplogEntryAttribution::Entity(search_context) = &search.entries[0].attribution else {
        panic!("search result without its Start must retain entity attribution");
    };
    assert_eq!(search_context.invocation.start_index, tool_start);
    assert_eq!(search_context.ancestors[0].start_index, middleware_start);
}

#[test]
async fn explicit_entity_attribution_rejects_non_causal_and_non_entity_anchors() {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            blob_storage,
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId::new(),
        agent_id: "Agent(\"invalid-entity-attribution\")".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();

    let non_start = oplog.add(OplogEntry::recovery_succeeded()).await.unwrap();
    let non_entity_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("not-an-entity".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    let entity_request = test_entity_request(
        &owned_agent_id,
        AgentEntity::Tool(ToolName::try_from("valid").unwrap()),
        EntityCallMode::Synchronous,
        None,
        "input".to_string().into_typed_schema_value().unwrap(),
    );
    let entity_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::GolemEntityInvoke,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(entity_request))),
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    let valid = oplog
        .add(OplogEntry::remove_retry_policy(
            Some(entity_start),
            "test-policy".to_string(),
        ))
        .await
        .unwrap();
    let invalid_non_start = oplog
        .add(OplogEntry::remove_retry_policy(
            Some(non_start),
            "test-policy".to_string(),
        ))
        .await
        .unwrap();
    let invalid_non_entity = oplog
        .add(OplogEntry::remove_retry_policy(
            Some(non_entity_start),
            "test-policy".to_string(),
        ))
        .await
        .unwrap();
    let invalid_forward_index = invalid_non_entity.next();
    let future_entity_start = invalid_forward_index.next();
    assert_eq!(
        oplog
            .add(OplogEntry::remove_retry_policy(
                Some(future_entity_start),
                "test-policy".to_string()
            ))
            .await
            .unwrap(),
        invalid_forward_index
    );
    assert_eq!(
        oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::GolemEntityInvoke,
                invocation_id: None,
                observational_owner: None,
                request: None,
                durable_function_type: DurableFunctionType::WriteLocal,
                span_started: None,
                denial: None,
            })
            .await
            .unwrap(),
        future_entity_start
    );
    oplog.commit(CommitLevel::Always).await.unwrap();

    let components: Arc<dyn ComponentService> = Arc::new(PanicComponentService);
    let valid_chunk = get_public_oplog_chunk(
        components.clone(),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        None,
        ComponentRevision::INITIAL,
        valid,
        1,
    )
    .await
    .unwrap();
    assert!(matches!(
        valid_chunk.entries[0].attribution,
        PublicOplogEntryAttribution::Entity(_)
    ));

    for (index, expected_error) in [
        (invalid_non_start, "does not reference a Start"),
        (invalid_non_entity, "is not an entity invocation"),
        (
            invalid_forward_index,
            "has non-causal entity parent Start index",
        ),
    ] {
        let result = get_public_oplog_chunk(
            components.clone(),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            None,
            ComponentRevision::INITIAL,
            index,
            1,
        )
        .await;
        let Err(error) = result else {
            panic!("expected invalid entity attribution at {index} to fail");
        };
        assert!(
            error.to_string().contains(expected_error),
            "expected {expected_error:?}, got {error}"
        );
    }
}

/// Renders P3 host call oplog entries (`P3HttpClientSend`,
/// `P3HttpClientConsumeBody`/`Chunk`, P3 sockets, keyvalue and blobstore
/// streams) through the public oplog API (`Start`/`End`/`Cancelled` entries
/// with typed-schema payloads), round-trips them through the gRPC protobuf
/// conversion used by the `golem worker oplog` transport path, and converts
/// them through the WIT representation used by the in-component oplog API.
#[test]
async fn p3_payloads_render_through_public_oplog_api_and_wit() {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            blob_storage,
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId(Uuid::new_v4()),
        agent_id: "public-oplog-p3".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();

    let cases: Vec<(HostFunctionName, HostRequest, HostResponse)> = vec![
        (
            HostFunctionName::P3HttpClientSend,
            HostRequestP3HttpClientSend {
                request: SerializableP3HttpClientSend {
                    method: SerializableHttpMethod::Post,
                    scheme: Some(SerializableP3HttpScheme::Https),
                    authority: Some("example.com".to_string()),
                    path_with_query: Some("/things?q=1".to_string()),
                    headers: header_map("content-type", b"application/json"),
                    options: Some(SerializableP3HttpRequestOptions {
                        connect_timeout_nanos: Some(1_000_000_000),
                        first_byte_timeout_nanos: None,
                        between_bytes_timeout_nanos: None,
                    }),
                },
            }
            .into(),
            HostResponseP3HttpClientSendResult {
                result: SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody {
                    headers: SerializableResponseHeaders {
                        status: 200,
                        headers: header_map("content-length", b"123"),
                    },
                    recording_complete_at_end: true,
                },
            }
            .into(),
        ),
        (
            HostFunctionName::P3HttpClientSend,
            HostRequestP3HttpClientSend {
                request: SerializableP3HttpClientSend {
                    method: SerializableHttpMethod::Get,
                    scheme: Some(SerializableP3HttpScheme::Http),
                    authority: Some("localhost:9999".to_string()),
                    path_with_query: None,
                    headers: HashMap::new(),
                    options: None,
                },
            }
            .into(),
            HostResponseP3HttpClientSendResult {
                result: SerializableP3HttpClientSendResult::HttpError(
                    SerializableHttpErrorCode::ConnectionRefused,
                ),
            }
            .into(),
        ),
        (
            HostFunctionName::P3HttpClientConsumeBody,
            HostRequestNoInput {}.into(),
            HostResponseP3HttpClientConsumeBodyResult {
                result: SerializableP3HttpConsumeBodyResult::Trailers(Some(header_map(
                    "x-trailer",
                    b"trailer-value",
                ))),
            }
            .into(),
        ),
        (
            HostFunctionName::P3HttpClientConsumeBodyChunk,
            HostRequestNoInput {}.into(),
            HostResponseP3HttpClientConsumeBodyChunk {
                chunk: SerializableP3HttpBodyChunk::Data(vec![1, 2, 3, 4]),
            }
            .into(),
        ),
        (
            HostFunctionName::P3SocketsTypesUdpSocketSend,
            HostRequestP3SocketsUdpSend {
                data: vec![1, 2, 3],
                remote_address: Some(SerializableP3IpSocketAddress {
                    address: SerializableIpAddress::IPv4 {
                        address: [127, 0, 0, 1],
                    },
                    port: 9000,
                    flow_info: None,
                    scope_id: None,
                }),
            }
            .into(),
            HostResponseP3SocketsUdpSend { result: Ok(()) }.into(),
        ),
        (
            HostFunctionName::P3SocketsTypesUdpSocketReceive,
            HostRequestNoInput {}.into(),
            HostResponseP3SocketsUdpReceive {
                result: Ok(SerializableP3UdpDatagram {
                    data: vec![4, 5, 6],
                    remote_address: SerializableP3IpSocketAddress {
                        address: SerializableIpAddress::IPv6 {
                            address: [0, 0, 0, 0, 0, 0, 0, 1],
                        },
                        port: 4242,
                        flow_info: Some(1),
                        scope_id: Some(2),
                    },
                }),
            }
            .into(),
        ),
        (
            HostFunctionName::P3SocketsTypesTcpSocketReceiveChunk,
            HostRequestNoInput {}.into(),
            HostResponseP3SocketsTcpReceiveChunk {
                chunk: SerializableP3TcpChunk::Data(vec![7, 8, 9]),
            }
            .into(),
        ),
        (
            HostFunctionName::P3SocketsTypesTcpSocketSendAcquire,
            HostRequestNoInput {}.into(),
            HostResponseP3SocketsTcpAcquire {
                result: Err(SerializableP3SocketErrorCode::ConnectionReset),
            }
            .into(),
        ),
        (
            HostFunctionName::P3KeyvalueTypesIncomingValueConsumeAsync,
            HostRequestNoInput {}.into(),
            HostResponseP3KeyvalueIncomingValueStream {
                contents: b"kv-value".to_vec(),
            }
            .into(),
        ),
        (
            HostFunctionName::P3BlobstoreTypesIncomingValueConsumeAsync,
            HostRequestNoInput {}.into(),
            HostResponseP3BlobstoreIncomingValueStream {
                contents: b"blob-value".to_vec(),
            }
            .into(),
        ),
    ];

    let mut expected_starts: BTreeMap<OplogIndex, (String, TypedSchemaValue)> = BTreeMap::new();
    let mut expected_ends: BTreeMap<OplogIndex, TypedSchemaValue> = BTreeMap::new();

    for (function_name, request, response) in cases {
        let expected_name = function_name.to_string();
        let expected_request = request.clone().into_typed_schema_value().unwrap();
        let expected_response = response.clone().into_typed_schema_value().unwrap();
        let (start_idx, end_idx) = oplog
            .add_completed_host_call(
                function_name,
                &request,
                &response,
                DurableFunctionType::WriteRemote,
                None,
                None,
            )
            .await
            .unwrap();
        expected_starts.insert(start_idx, (expected_name, expected_request));
        expected_ends.insert(end_idx, expected_response);
    }

    // Generic entity calls already carry a self-contained typed terminal. The public oplog must
    // expose that terminal directly rather than schema-encoding a TypedSchemaValue inside another
    // TypedSchemaValue, which exceeds protobuf's recursion limit for realistic tool terminals.
    let entity_input = "entity-input"
        .to_string()
        .into_typed_schema_value()
        .unwrap();
    let entity_terminal = SerializableToolOperationTerminal {
        body_execution: SerializableEntityBodyExecution::Executed,
        result: Ok(SerializableToolStructuredResult { result: None }),
        admission_refusal: None,
    }
    .into_typed_schema_value()
    .unwrap();
    let entity_request: HostRequest = HostRequestEntityInvocation {
        metadata: vec![1, 2, 3],
        input: entity_input.clone(),
        stream_session_idempotency_key: golem_common::model::IdempotencyKey::new(
            "entity-stream-session".to_string(),
        ),
    }
    .into();
    let entity_response: HostResponse = HostResponseEntityInvocation {
        result: Ok(entity_terminal.clone()),
    }
    .into();
    let (entity_start_idx, entity_end_idx) = oplog
        .add_completed_host_call(
            HostFunctionName::GolemEntityInvoke,
            &entity_request,
            &entity_response,
            DurableFunctionType::WriteLocal,
            None,
            None,
        )
        .await
        .unwrap();
    expected_starts.insert(
        entity_start_idx,
        (
            HostFunctionName::GolemEntityInvoke.to_string(),
            entity_input,
        ),
    );
    expected_ends.insert(entity_end_idx, entity_terminal);

    let mut input_type = SchemaType::string();
    let mut input_value = SchemaValue::String("nested MCP input".into());
    for _ in 0..20 {
        input_type = SchemaType::list(input_type);
        input_value = SchemaValue::List {
            elements: vec![input_value],
        };
    }
    let input = TypedSchemaValue::new(SchemaGraph::anonymous(input_type), input_value);
    let request: HostRequest = golem_common::model::oplog::HostRequestMcpToolCall {
        input: input.clone(),
    }
    .into();
    let response: HostResponse = golem_common::model::oplog::HostResponseMcpToolCall {
        result: Ok(b"{\"content\":[]}".to_vec()),
    }
    .into();
    let expected_response = response.clone().into_typed_schema_value().unwrap();
    let (start, end) = oplog
        .add_completed_host_call(
            HostFunctionName::McpToolCall,
            &request,
            &response,
            DurableFunctionType::WriteRemote,
            None,
            None,
        )
        .await
        .unwrap();
    expected_starts.insert(start, (HostFunctionName::McpToolCall.to_string(), input));
    expected_ends.insert(end, expected_response);

    // A host call terminated by `Cancelled` instead of `End`: a standalone
    // `Start` for a consume-body-chunk call, cancelled with a matching
    // partial P3 payload — the sequence the executor emits when a durable
    // call is cancelled mid-flight.
    let cancelled_request: HostRequest = HostRequestNoInput {}.into();
    let expected_cancelled_request = cancelled_request.clone().into_typed_schema_value().unwrap();
    let cancelled_request_payload = oplog_service
        .upload_payload(&owned_agent_id, AgentMode::Durable, &cancelled_request)
        .await
        .unwrap();
    let cancelled_start_index = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::P3HttpClientConsumeBodyChunk,
            invocation_id: None,
            observational_owner: None,
            request: Some(cancelled_request_payload),
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    expected_starts.insert(
        cancelled_start_index,
        (
            HostFunctionName::P3HttpClientConsumeBodyChunk.to_string(),
            expected_cancelled_request,
        ),
    );

    let partial: HostResponse = HostResponseP3HttpClientConsumeBodyChunk {
        chunk: SerializableP3HttpBodyChunk::Cancelled,
    }
    .into();
    let expected_partial = partial.clone().into_typed_schema_value().unwrap();
    let partial_payload = oplog_service
        .upload_payload(&owned_agent_id, AgentMode::Durable, &partial)
        .await
        .unwrap();
    oplog
        .add(OplogEntry::cancelled(
            cancelled_start_index,
            Some(partial_payload),
            None,
        ))
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let last_index = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await
        .unwrap();
    let raw_entries = oplog_service
        .read_exact(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::INITIAL,
            Into::<u64>::into(last_index),
        )
        .await
        .unwrap();

    let components: Arc<dyn ComponentService> = Arc::new(PanicComponentService);

    let mut seen_starts = 0;
    let mut seen_ends = 0;
    let mut seen_cancelled = 0;
    for (index, raw_entry) in raw_entries {
        let public_entry = PublicOplogEntry::from_oplog_entry(
            index,
            raw_entry,
            oplog_service.clone(),
            components.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            None,
            ComponentRevision::new(1).unwrap(),
        )
        .await
        .unwrap_or_else(|err| panic!("rendering oplog entry {index} failed: {err}"));

        match &public_entry {
            PublicOplogEntry::Start(params) => {
                let (expected_name, expected_request) = expected_starts
                    .get(&index)
                    .unwrap_or_else(|| panic!("unexpected Start entry at {index}"));
                assert_eq!(&params.function_name, expected_name);
                assert_eq!(params.request.as_ref(), Some(expected_request));
                seen_starts += 1;
            }
            PublicOplogEntry::End(params) => {
                let expected_response = expected_ends
                    .get(&index)
                    .unwrap_or_else(|| panic!("unexpected End entry at {index}"));
                assert_eq!(params.response.as_ref(), Some(expected_response));
                assert_eq!(params.start_index.next(), index);
                seen_ends += 1;
            }
            PublicOplogEntry::Cancelled(params) => {
                assert_eq!(params.start_index, cancelled_start_index);
                assert_eq!(params.partial.as_ref(), Some(&expected_partial));
                seen_cancelled += 1;
            }
            other => panic!("unexpected public oplog entry at {index}: {other:?}"),
        }

        // The same entries must survive the gRPC protobuf round-trip: this is
        // the transport boundary between the worker executor and the worker
        // service, i.e. the path `golem worker oplog` output travels through.
        let proto_entry: golem_api_grpc::proto::golem::worker::OplogEntry = public_entry
            .clone()
            .try_into()
            .unwrap_or_else(|err| panic!("protobuf conversion of entry {index} failed: {err}"));
        let encoded = proto_entry.encode_to_vec();
        let proto_entry =
            golem_api_grpc::proto::golem::worker::OplogEntry::decode(encoded.as_slice())
                .unwrap_or_else(|err| panic!("protobuf decoding of entry {index} failed: {err}"));
        let round_tripped: PublicOplogEntry = proto_entry
            .try_into()
            .unwrap_or_else(|err| panic!("public entry conversion at {index} failed: {err}"));
        assert_eq!(round_tripped, public_entry);

        // They must also survive the WIT conversion used by the in-component
        // oplog API (oplog processors / golem-api)
        let wit_entry: Result<crate::preview2::golem_api_1_x::oplog::PublicOplogEntry, String> =
            public_entry.try_into();
        wit_entry
            .unwrap_or_else(|err| panic!("WIT conversion of oplog entry {index} failed: {err}"));
    }

    assert_eq!(seen_starts, expected_starts.len());
    assert_eq!(seen_ends, expected_ends.len());
    assert_eq!(seen_cancelled, 1);
}

#[test]
fn mcp_discovery_metadata_roundtrips_public_oplog_without_recursive_type_values() {
    use golem_common::model::mcp_import::mcp_import_bridge_source;
    use golem_common::model::oplog::payload::types::{
        SerializableDiscoveredTools, SerializableMcpImportDiscovery,
        SerializableToolDiscoverySnapshot,
    };
    use golem_common::model::oplog::{HostResponseGolemToolTool, HostResponseGolemToolTools};
    use golem_common::schema::tool::DiscoveredTool;
    use golem_common::schema::{FromSchema, IntoSchema, SchemaValue};
    use golem_mcp_import::tool::{Limits, ProjectedTool};
    use serde_json::json;

    #[derive(FromSchema)]
    struct DiscoveryResponse {
        result: Result<SerializableToolDiscoverySnapshot, String>,
    }

    let mut tools = Vec::new();
    for depth in [0, 40] {
        let mut output = json!({"type":"string"});
        for _ in 0..depth {
            output = json!({"type":"object","properties":{"nested":output},"required":["nested"],"additionalProperties":false});
        }
        let tool = ProjectedTool::new(
            &json!({
                "name":"lookup",
                "description":"quoted \"text\" and \\ path — λ",
                "inputSchema":{
                    "type":"object", "properties":{"query":{"$ref":"#/$defs/Query"}},
                    "required":["query"], "additionalProperties":false,
                    "$defs":{"Query":{"type":"string"}}
                },
                "outputSchema":output
            }),
            "lookup",
            Limits::default(),
        )
        .unwrap();
        tools.push(DiscoveredTool::new(
            tool.definition,
            mcp_import_bridge_source(),
        ));
    }
    let serialized_tools = SerializableDiscoveredTools(tools);
    assert!(matches!(
        serialized_tools.to_value(),
        SchemaValue::String(_)
    ));
    assert_eq!(
        SerializableDiscoveredTools::from_value(&serialized_tools.to_value()).unwrap(),
        serialized_tools,
    );
    assert!(
        SerializableDiscoveredTools::from_value(&SchemaValue::String("[] trailing".into()))
            .is_err()
    );
    assert!(SerializableDiscoveredTools::from_value(&SchemaValue::String("{".into())).is_err());
    assert!(
        SerializableDiscoveredTools::from_value(&SchemaValue::List {
            elements: Vec::new()
        })
        .is_err()
    );
    let snapshot = SerializableToolDiscoverySnapshot {
        deployment_revision: Some(37),
        dynamic_tools: vec![
            SerializableMcpImportDiscovery {
                import_index: 0,
                tools: serialized_tools.clone(),
                exclusions: Vec::new(),
            },
            SerializableMcpImportDiscovery {
                import_index: 1,
                tools: SerializableDiscoveredTools::default(),
                exclusions: vec![("excluded".into(), "unrepresentable schema".into())],
            },
            SerializableMcpImportDiscovery {
                import_index: 2,
                tools: serialized_tools,
                exclusions: Vec::new(),
            },
        ],
    };
    for result in [
        Ok(snapshot),
        Ok(SerializableToolDiscoverySnapshot {
            deployment_revision: None,
            dynamic_tools: Vec::new(),
        }),
        Err("registry unavailable".into()),
    ] {
        for response in [
            HostResponse::from(HostResponseGolemToolTools {
                result: result.clone(),
            }),
            HostResponse::from(HostResponseGolemToolTool {
                result: result.clone(),
            }),
        ] {
            let public_entry = PublicOplogEntry::End(EndParams {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::INITIAL,
                response: Some(host_response_to_public_value(response).unwrap()),
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            });
            let proto: golem_api_grpc::proto::golem::worker::OplogEntry =
                public_entry.clone().try_into().unwrap();
            let decoded = golem_api_grpc::proto::golem::worker::OplogEntry::decode(
                proto.encode_to_vec().as_slice(),
            )
            .unwrap();
            let roundtripped: PublicOplogEntry = decoded.try_into().unwrap();
            assert_eq!(roundtripped, public_entry);
            let PublicOplogEntry::End(end) = roundtripped else {
                panic!("expected End");
            };
            let restored = DiscoveryResponse::from_value(end.response.unwrap().value()).unwrap();
            assert_eq!(restored.result, result);
            let _: crate::preview2::golem_api_1_x::oplog::PublicOplogEntry =
                public_entry.try_into().unwrap();
        }
    }
}

async fn open_test_oplog(
    name: &str,
) -> (
    Arc<PrimaryOplogService>,
    OwnedAgentId,
    Arc<dyn crate::services::oplog::Oplog>,
) {
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: golem_common::model::component::ComponentId(Uuid::new_v4()),
        agent_id: name.to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let oplog = oplog_service
        .open(
            &mut oplog_service.lock_lifecycle(&owned_agent_id.agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();
    (oplog_service, owned_agent_id, oplog)
}

fn log_entry(message_size: usize) -> OplogEntry {
    OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        level: LogLevel::Stdout,
        context: "context".to_string(),
        message: "x".repeat(message_size),
        trace_context: None,
    }
}

fn page_limits(max_bytes: usize, max_entry_bytes: usize) -> PublicOplogPageLimits {
    PublicOplogPageLimits {
        count: 10,
        max_bytes,
        max_entry_bytes,
    }
}

fn public_page_entries(
    entries: &[golem_api_grpc::proto::golem::worker::OplogPageEntry],
) -> Vec<PublicOplogPageEntry> {
    entries
        .iter()
        .map(|entry| PublicOplogPageEntry::try_from(entry.clone()).unwrap())
        .collect()
}

fn page_indices(
    entries: &[golem_api_grpc::proto::golem::worker::OplogPageEntry],
) -> Vec<(OplogIndex, bool)> {
    public_page_entries(entries)
        .iter()
        .map(|entry| match entry {
            PublicOplogPageEntry::Included(entry) => (entry.oplog_index, true),
            PublicOplogPageEntry::Omitted(omitted) => (omitted.oplog_index, false),
        })
        .collect()
}

#[test]
async fn oplog_pages_omit_entries_larger_than_the_entry_limit() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("omit-large-entries").await;
    let small_index = oplog.add(log_entry(10)).await.unwrap();
    let large_payload = vec![0xAB; 5000];
    let snapshot_index = oplog
        .add(OplogEntry::Snapshot {
            timestamp: Timestamp::now_utc(),
            data: oplog.upload_payload(&large_payload).await.unwrap(),
            mime_type: "application/octet-stream".to_string(),
            active_cards: Vec::new(),
            wallet_generation: 0,
            filesystem_snapshot: None,
        })
        .await
        .unwrap();
    let large_log_index = oplog.add(log_entry(5000)).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    let page_revision = ComponentRevision::new(5).unwrap();

    let page = get_public_oplog_page(
        Arc::new(PanicComponentService),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        None,
        page_revision,
        OplogIndex::INITIAL,
        page_limits(1_000_000, 1000),
    )
    .await
    .unwrap();
    assert_eq!(
        page_indices(&page.entries),
        vec![
            (small_index, true),
            (snapshot_index, false),
            (large_log_index, false)
        ]
    );

    for (page_entry, entry_type) in public_page_entries(&page.entries)[1..]
        .iter()
        .zip(["Snapshot", "Log"])
    {
        let PublicOplogPageEntry::Omitted(omitted) = page_entry else {
            unreachable!()
        };
        let full = get_public_oplog_entry(
            Arc::new(PanicComponentService),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            None,
            omitted.oplog_index,
            Some(omitted.component_revision),
        )
        .await
        .unwrap();
        let full_size = prost::Message::encoded_len(
            &golem_api_grpc::proto::golem::worker::OplogEntryWithIndex::try_from(full.clone())
                .unwrap(),
        );
        assert_eq!(omitted.entry_type, entry_type);
        assert_eq!(full.entry.entry_type(), entry_type);
        assert_eq!(omitted.timestamp, full.entry.timestamp());
        assert_eq!(omitted.attribution, full.attribution);
        assert_eq!(omitted.size, full_size as u64);
        assert_eq!(omitted.component_revision, page_revision);
        assert!(omitted.size > 5000);
    }

    let PublicOplogEntry::Snapshot(snapshot) = get_public_oplog_entry(
        Arc::new(PanicComponentService),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        None,
        snapshot_index,
        None,
    )
    .await
    .unwrap()
    .entry
    else {
        panic!("expected the snapshot entry");
    };
    assert_eq!(
        snapshot.data,
        golem_common::model::oplog::PublicSnapshotData::Raw(
            golem_common::model::oplog::RawSnapshotData {
                data: large_payload,
                mime_type: "application/octet-stream".to_string(),
            }
        )
    );

    assert!(
        get_public_oplog_entry(
            Arc::new(PanicComponentService),
            oplog_service,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            large_log_index.next(),
            None,
        )
        .await
        .is_err()
    );
}

#[test]
async fn oplog_pages_end_at_the_byte_budget_and_resume_at_the_next_entry() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("page-byte-budget").await;
    let mut indices = Vec::new();
    for size in [1000, 1000, 5000, 1000] {
        indices.push(oplog.add(log_entry(size)).await.unwrap());
    }
    oplog.commit(CommitLevel::Always).await.unwrap();

    let read_page = |from: OplogIndex| {
        get_public_oplog_page(
            Arc::new(PanicComponentService),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            None,
            ComponentRevision::INITIAL,
            from,
            page_limits(2500, 1_000_000),
        )
    };

    let first = read_page(OplogIndex::INITIAL).await.unwrap();
    assert_eq!(
        page_indices(&first.entries),
        vec![(indices[0], true), (indices[1], true)]
    );
    assert_eq!(first.next_oplog_index, indices[2]);

    // An entry larger than the budget still makes a page on its own.
    let second = read_page(first.next_oplog_index).await.unwrap();
    assert_eq!(page_indices(&second.entries), vec![(indices[2], true)]);
    assert_eq!(second.next_oplog_index, indices[3]);

    let third = read_page(second.next_oplog_index).await.unwrap();
    assert_eq!(page_indices(&third.entries), vec![(indices[3], true)]);
    assert_eq!(third.next_oplog_index, indices[3].next());
}

#[test]
async fn oplog_search_pages_share_the_byte_budget_across_chunks_and_count_only_matches() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("search-byte-budget").await;
    let mut matching = Vec::new();
    for _ in 0..3 {
        oplog.add(log_entry(0)).await.unwrap();
        oplog.add(log_entry(2000)).await.unwrap();
        matching.push(
            oplog
                .add(OplogEntry::Log {
                    timestamp: Timestamp::now_utc(),
                    parent_start_index: None,
                    level: LogLevel::Stderr,
                    context: "context".to_string(),
                    message: format!("needle {}", "y".repeat(1000)),
                    trace_context: None,
                })
                .await
                .unwrap(),
        );
    }
    oplog.commit(CommitLevel::Always).await.unwrap();

    let search = |from: OplogIndex| {
        search_public_oplog_page(
            Arc::new(PanicComponentService),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            None,
            ComponentRevision::INITIAL,
            from,
            PublicOplogPageLimits {
                count: 3,
                max_bytes: 2500,
                max_entry_bytes: 1_000_000,
            },
            "needle",
        )
    };

    // Every internal chunk reads three entries. The 2000 byte log entries do not match and do
    // not count against the budget, so the first two matches fit, from different chunks, and
    // the third match ends the page.
    let first = search(OplogIndex::INITIAL).await.unwrap();
    assert_eq!(
        page_indices(&first.entries),
        vec![(matching[0], true), (matching[1], true)]
    );
    assert_eq!(first.next_oplog_index, matching[2]);

    let second = search(first.next_oplog_index).await.unwrap();
    assert_eq!(page_indices(&second.entries), vec![(matching[2], true)]);
}

#[test]
async fn component_revision_lookup_reads_past_its_batch_size() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("revision-lookup").await;
    let mut indices = Vec::new();
    for i in 0..600 {
        let entry = if i == 300 {
            OplogEntry::SuccessfulUpdate {
                timestamp: Timestamp::now_utc(),
                target_revision: ComponentRevision::new(5).unwrap(),
                new_component_size: 100,
                new_total_linear_memory_size: None,
                new_active_plugins: HashSet::new(),
                snapshot_assisted_details: None,
            }
        } else {
            log_entry(0)
        };
        indices.push(oplog.add(entry).await.unwrap());
    }
    oplog.commit(CommitLevel::Always).await.unwrap();

    let revision_at = |index: OplogIndex| {
        find_component_revision_at(
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            index,
        )
    };
    // The lookup covers the entries before `start`, so the update applies from the next entry.
    assert_eq!(
        revision_at(indices[300]).await.unwrap(),
        ComponentRevision::INITIAL
    );
    assert_eq!(
        revision_at(indices[301]).await.unwrap(),
        ComponentRevision::new(5).unwrap()
    );
    assert_eq!(
        revision_at(indices[599].next().next()).await.unwrap(),
        ComponentRevision::new(5).unwrap()
    );
    assert_eq!(
        revision_at(OplogIndex::NONE).await.unwrap(),
        ComponentRevision::INITIAL
    );
}
