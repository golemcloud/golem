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
use crate::services::golem_config::ScanBudget;
use crate::services::oplog::{CommitLevel, OplogOps, PrimaryOplogService};
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::AgentTypeName;

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
    HostResponseP3SocketsTcpReceiveChunk, HostResponseP3SocketsTcpSendChunk,
    HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend, HostResponseSecretRevealed,
    HostStreamKind, LogLevel, OplogPayload,
};
use golem_common::model::oplog::{OplogCursor, OplogScanAllowance};
use golem_common::model::regions::OplogRegion;
use golem_common::model::tool::{
    CompiledToolBinding, SecretKeyScope, ToolFilesystemAccess, ToolName, ToolProvisionConfig,
    ToolSource,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationPayload, AgentMetadata, AgentStatusRecord,
    RetryConfig, Timestamp, TransactionId,
};
use golem_common::read_only_lock;
use golem_common::schema::tool::{CommandTree, Tool};
use golem_common::schema::{
    IntoTypedSchemaValue, SchemaGraph, SchemaType, SchemaValue, SecretValuePayload,
};
use golem_service_base::error::worker_executor::StaleCursorReason;
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
    use golem_common::model::OplogIndex;
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
            logical_invocation: uuid::Uuid::new_v4().into(),
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
        assert_eq!(
            golem_common::model::oplog::OplogEnrichmentEntry::try_from(wire).unwrap(),
            raw.clone().into()
        );
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
        assert!(golem_common::model::oplog::OplogEnrichmentEntry::try_from(wire).is_err());
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
            assert_eq!(
                golem_common::model::oplog::OplogEnrichmentEntry::try_from(wire).unwrap(),
                external.into()
            );
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
        assert!(
            golem_common::model::oplog::OplogEnrichmentEntry::try_from(wrong_ownership).is_err()
        );
        let public_raw =
            golem_common::model::oplog::OplogEnrichmentEntry::try_from(public.clone()).unwrap();
        let golem_common::model::oplog::OplogEnrichmentEntry::Other(
            OplogEntry::ToolAttachmentRead {
                observation: restored_observation,
                payload: restored_payload,
                entity_parent_start_index,
                ..
            },
        ) = public_raw
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
    assert!(golem_common::model::oplog::OplogEnrichmentEntry::try_from(raw).is_err());
}

#[test]
fn stream_terminal_public_wit_preserves_terminal_payloads() {
    use crate::preview2::golem_api_1_x::oplog;
    use golem_common::model::durable_stream::{
        LocalStreamId, StreamError, StreamOffset, StreamTerminal, StreamTerminalAuthor,
        StreamTerminalOrigin, StreamTerminalRecord,
    };
    let declared = (17u32, "declared failure".to_string())
        .into_typed_schema_value()
        .unwrap();
    for (origin, terminal) in [
        StreamTerminal::Finished,
        StreamTerminal::Failed(StreamError::Failed(Box::new(declared))),
        StreamTerminal::Failed(StreamError::Trapped("guest trap".into())),
        StreamTerminal::Failed(StreamError::Interrupted("session deleted".into())),
        StreamTerminal::Failed(StreamError::ResourceExhausted("retention limit".into())),
        StreamTerminal::ConsumerCancelled {
            details: Some("reader dropped".into()),
        },
    ]
    .into_iter()
    .flat_map(|terminal| {
        [StreamTerminalOrigin::Source, StreamTerminalOrigin::Control]
            .map(|origin| (origin, terminal.clone()))
    }) {
        let record = StreamTerminalRecord {
            format_version: 1,
            stream_id: LocalStreamId(OplogIndex::from_u64(4)),
            sequence: 3,
            offset: StreamOffset::new(OplogIndex::from_u64(8), 0),
            authored_by: StreamTerminalAuthor::Host,
            origin,
            terminal,
        }
        .into_typed_schema_value()
        .unwrap();
        let public = PublicOplogEntry::StreamTerminal(StreamTerminalParams {
            timestamp: Timestamp::now_utc().rounded(),
            record: record.clone(),
        });
        let wire: oplog::PublicOplogEntry = public.try_into().unwrap();
        let oplog::PublicOplogEntry::StreamTerminal(wire) = wire else {
            panic!("wrong public variant")
        };
        assert_eq!(
            golem_schema::schema::wit::decode_typed(&wire.record).unwrap(),
            record
        );
    }
}

#[test]
fn raw_stream_telemetry_survives_uncached_payload_roundtrips() {
    use crate::preview2::golem_api_1_x::oplog;
    use golem_common::model::durable_stream::StreamTerminalOrigin;
    use golem_common::model::oplog::{
        DurableStreamEventSummary as Summary, DurableStreamOutcome as Outcome,
        DurableStreamTerminalOutcome as TerminalOutcome, PayloadId,
    };

    for (external, origin) in [false, true].into_iter().flat_map(|external| {
        [StreamTerminalOrigin::Source, StreamTerminalOrigin::Control]
            .map(|origin| (external, origin))
    }) {
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
                StreamTerminal,
                Some(Summary::Terminal {
                    outcome: TerminalOutcome::Finished,
                    origin,
                })
            ),
            entry!(
                StreamTerminal,
                Some(Summary::Terminal {
                    outcome: TerminalOutcome::Failed,
                    origin,
                })
            ),
            entry!(
                StreamTerminal,
                Some(Summary::Terminal {
                    outcome: TerminalOutcome::ConsumerCancelled,
                    origin,
                })
            ),
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
            let restored =
                golem_common::model::oplog::OplogEnrichmentEntry::try_from(wire).unwrap();
            assert_eq!(restored, entry.into());
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
        assert_eq!(
            golem_common::model::oplog::OplogEnrichmentEntry::try_from(wire).unwrap(),
            entry.into()
        );
    }
}

fn span_closed(
    entity_parent_start_index: Option<OplogIndex>,
    resource_start_index: OplogIndex,
) -> OplogEntry {
    let timestamp = Timestamp::now_utc().rounded();
    OplogEntry::SpanClosed {
        timestamp,
        entity_parent_start_index,
        resource_start_index: Some(resource_start_index),
        span_finished: SpanFinished {
            span_id: SpanId::generate(),
            finished_at: timestamp,
            outcome: SpanOutcome::Completed,
        },
    }
}

#[test]
fn raw_span_closed_survives_wit_roundtrip() {
    use crate::preview2::golem_api_1_x::oplog;

    for entity_parent_start_index in [None, Some(OplogIndex::from_u64(4))] {
        let entry = span_closed(entity_parent_start_index, OplogIndex::from_u64(9));
        let wire: oplog::OplogEntry = entry.clone().try_into().unwrap();
        assert_eq!(
            golem_common::model::oplog::OplogEnrichmentEntry::try_from(wire).unwrap(),
            entry.into()
        );
    }
}

/// Component service stub for entries whose rendering must not need component
/// metadata (`Start`/`End`/`Cancelled` host call entries).
struct PanicComponentService;

fn test_tool_definition() -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        http_mount: None,
        commands: CommandTree { nodes: Vec::new() },
        schema: SchemaGraph::empty(),
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
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

fn test_component_id() -> ComponentId {
    ComponentId(Uuid::from_u128(0xc0))
}

fn make_agent_metadata(
    agent_id: AgentId,
    created_by: AccountId,
    environment_id: EnvironmentId,
) -> AgentMetadata {
    AgentMetadata {
        component_id: test_component_id(),
        agent_type_name: None,
        canonical_key: None,
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
        constructor_parameters: None,
        fingerprint: AgentFingerprint::new(),
        agent_mode: AgentMode::Durable,
        create_request: golem_common::model::oplog::CreateRequest::new(
            golem_common::model::oplog::CreateRequestKind::GetOrCreate,
            None,
            golem_common::model::caller::Caller::anonymous(),
            &golem_common::model::invocation_context::InvocationContextStack::fresh_rounded(),
        ),
    }
}

#[test]
async fn checkpoint_after_plugin_removal_uses_recorded_source_revision() {
    use golem_common::base_model::component_metadata::ComponentProvisionConfig;
    use golem_common::model::component::PluginPriority;
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
    use golem_common::model::plugin_registration::PluginRegistrationId;

    struct RevisionComponentService(Component);

    #[async_trait]
    #[golem_async_trait::region_simple]
    impl ComponentService for RevisionComponentService {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            panic!("rendering must not load executable components")
        }

        async fn get_metadata(
            &self,
            component_id: ComponentId,
            forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            assert_eq!(component_id, self.0.id);
            let mut component = self.0.clone();
            let revision = forced_revision.unwrap();
            if revision != component.revision {
                assert_eq!(revision, ComponentRevision::new(2).unwrap());
                component.revision = revision;
                component.metadata = component
                    .metadata
                    .with_component_provision_config(ComponentProvisionConfig::default());
            }
            Ok(component)
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: EnvironmentId,
            _resolving_application: golem_common::model::application::ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            panic!("rendering must use the supplied component identity")
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            Vec::new()
        }

        async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
    }

    let source_revision = ComponentRevision::INITIAL;
    let updated_revision = ComponentRevision::new(2).unwrap();
    let plugin = InstalledPlugin {
        environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
        priority: PluginPriority(0),
        parameters: BTreeMap::new(),
        plugin_registration_id: PluginRegistrationId::new(),
        plugin_name: "removed-plugin".to_string(),
        plugin_version: "1.0".to_string(),
        oplog_processor_component_id: Some(ComponentId::new()),
        oplog_processor_component_revision: Some(source_revision),
        installed_by: golem_common::model::account::AccountId::new(),
    };
    let component_id = ComponentId::new();
    let environment_id = EnvironmentId::new();
    let components = Arc::new(RevisionComponentService(Component {
        id: component_id,
        revision: source_revision,
        environment_id,
        component_name: ComponentName("source".to_string()),
        hash: golem_common::model::diff::Hash::empty(),
        application_id: golem_common::model::application::ApplicationId::new(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("test@golem"),
        application_name: "test-app".parse().unwrap(),
        environment_name: "test-env".parse().unwrap(),
        component_size: 100,
        metadata: ComponentMetadata::from_parts(
            Default::default(),
            Vec::new(),
            Vec::new(),
            BTreeMap::new(),
        )
        .with_component_provision_config(ComponentProvisionConfig {
            plugins: vec![plugin.clone()],
            ..Default::default()
        }),
        created_at: jiff::Timestamp::now(),
        wasm_hash: golem_common::model::diff::Hash::empty(),
        package_metadata: None,
        object_store_key: String::new(),
    }));
    let owner = OwnedAgentId::new(environment_id, &AgentId::from_parts(7, 1));
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let timestamp = Timestamp::from(1_000_u64);
    let update = PublicOplogEntry::from_oplog_entry(
        OplogIndex::from_u64(10),
        OplogEntry::SuccessfulUpdate {
            timestamp,
            target_revision: updated_revision,
            new_component_size: 100,
            new_total_linear_memory_size: None,
            new_active_plugins: Default::default(),
            snapshot_assisted_details: None,
            bootstrap: None,
        },
        oplog_service.clone(),
        components.clone(),
        &owner,
        AgentMode::Durable,
        component_id,
        OwnerKind::EphemeralExternalTool,
        None,
        source_revision,
        ClaimVisibility::Hidden,
    )
    .await
    .unwrap();
    assert!(
        matches!(update, PublicOplogEntry::SuccessfulUpdate(p) if p.new_active_plugins.is_empty())
    );

    let checkpoint = OplogEntry::OplogProcessorCheckpoint {
        timestamp,
        plugin_grant_id: plugin.environment_plugin_grant_id,
        source_component_revision: source_revision,
        target_agent_id: AgentId::random_reserved(),
        confirmed_up_to: OplogIndex::from_u64(8),
        sending_up_to: OplogIndex::from_u64(8),
        last_batch_start: OplogIndex::INITIAL,
        delivery: None,
    };
    let public = PublicOplogEntry::from_oplog_entry(
        OplogIndex::from_u64(11),
        checkpoint.clone(),
        oplog_service,
        components,
        &owner,
        AgentMode::Durable,
        component_id,
        OwnerKind::EphemeralExternalTool,
        None,
        updated_revision,
        ClaimVisibility::Hidden,
    )
    .await
    .unwrap();
    let PublicOplogEntry::OplogProcessorCheckpoint(params) = &public else {
        panic!("expected checkpoint")
    };
    assert_eq!(params.source_component_revision, source_revision);
    assert_eq!(
        params.plugin.environment_plugin_grant_id,
        plugin.environment_plugin_grant_id
    );
    assert_eq!(params.plugin.plugin_name, plugin.plugin_name);

    let proto: golem_api_grpc::proto::golem::worker::OplogEntry =
        public.clone().try_into().unwrap();
    assert_eq!(PublicOplogEntry::try_from(proto).unwrap(), public);
    let wit: crate::preview2::golem_api_1_x::oplog::PublicOplogEntry =
        public.clone().try_into().unwrap();
    let crate::preview2::golem_api_1_x::oplog::PublicOplogEntry::OplogProcessorCheckpoint(wit) =
        wit
    else {
        panic!("expected public checkpoint")
    };
    assert_eq!(wit.source_component_revision, u64::from(source_revision));
    let raw: golem_common::model::oplog::OplogEnrichmentEntry = public.try_into().unwrap();
    assert_eq!(raw, checkpoint.clone().into());
    let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        checkpoint.clone().try_into().unwrap();
    assert_eq!(OplogEntry::try_from(proto).unwrap(), checkpoint);
    let wit: crate::preview2::golem_api_1_x::oplog::OplogEntry =
        checkpoint.clone().try_into().unwrap();
    assert_eq!(
        golem_common::model::oplog::OplogEnrichmentEntry::try_from(wit).unwrap(),
        checkpoint.into()
    );
}

#[test]
async fn public_oplog_enrichment_uses_authoritative_kind_without_rewriting_create_projection() {
    use golem_common::base_model::component_metadata::{ComponentProvisionConfig, KnownExports};
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
    use golem_common::model::oplog::{CreateProjectionParameters, OplogEnrichmentEntry};
    use golem_common::model::plugin_registration::PluginRegistrationId;

    struct MetadataComponentService(Component);

    #[async_trait]
    #[golem_async_trait::region_simple]
    impl ComponentService for MetadataComponentService {
        async fn get(
            &self,
            _engine: &wasmtime::Engine,
            _component_id: ComponentId,
            _component_revision: ComponentRevision,
        ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
            panic!("enrichment must not load executable components")
        }

        async fn get_metadata(
            &self,
            component_id: ComponentId,
            forced_revision: Option<ComponentRevision>,
        ) -> Result<Component, WorkerExecutorError> {
            assert_eq!(component_id, self.0.id);
            assert_eq!(forced_revision, Some(self.0.revision));
            Ok(self.0.clone())
        }

        async fn resolve_component(
            &self,
            _component_reference: String,
            _resolving_environment: EnvironmentId,
            _resolving_application: golem_common::model::application::ApplicationId,
            _resolving_account: AccountId,
        ) -> Result<Option<ComponentId>, WorkerExecutorError> {
            panic!("enrichment must use the supplied component identity")
        }

        async fn all_cached_metadata(&self) -> Vec<Component> {
            Vec::new()
        }

        async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
    }

    let component_id = ComponentId::new();
    let grant_id = EnvironmentPluginGrantId::new();
    let revision = ComponentRevision::INITIAL;
    let environment_id = EnvironmentId::new();
    let owner = OwnedAgentId::new(environment_id, &AgentId::random_reserved());
    let components = Arc::new(MetadataComponentService(Component {
        id: component_id,
        revision,
        environment_id,
        component_name: ComponentName("processor".to_string()),
        hash: golem_common::model::diff::Hash::empty(),
        application_id: golem_common::model::application::ApplicationId::new(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("test@golem"),
        application_name: golem_common::model::application::ApplicationName::try_from("test-app")
            .unwrap(),
        environment_name: golem_common::model::environment::EnvironmentName::try_from("test-env")
            .unwrap(),
        component_size: 100,
        metadata: ComponentMetadata::from_parts(
            KnownExports {
                oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
                ..Default::default()
            },
            Vec::new(),
            Vec::new(),
            BTreeMap::new(),
        )
        .with_component_provision_config(ComponentProvisionConfig {
            plugins: vec![InstalledPlugin {
                environment_plugin_grant_id: grant_id,
                priority: golem_common::model::component::PluginPriority(0),
                parameters: BTreeMap::new(),
                plugin_registration_id: PluginRegistrationId::new(),
                plugin_name: "component-plugin".to_string(),
                plugin_version: "1.0".to_string(),
                oplog_processor_component_id: Some(ComponentId::new()),
                oplog_processor_component_revision: Some(revision),
                installed_by: golem_common::model::account::AccountId::new(),
            }],
            ..Default::default()
        }),
        created_at: jiff::Timestamp::now(),
        wasm_hash: golem_common::model::diff::Hash::empty(),
        package_metadata: None,
        object_store_key: String::new(),
    }));
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let timestamp = Timestamp::now_utc();
    for owner_kind in [OwnerKind::OplogProcessor, OwnerKind::EphemeralExternalTool] {
        let updated = PublicOplogEntry::from_oplog_entry(
            OplogIndex::INITIAL,
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_revision: revision,
                new_component_size: 101,
                new_total_linear_memory_size: Some(0),
                new_active_plugins: [grant_id].into_iter().collect(),
                snapshot_assisted_details: None,
                bootstrap: Some(OplogRegion {
                    start: OplogIndex::from_u64(3),
                    end: OplogIndex::from_u64(7),
                }),
            },
            oplog_service.clone(),
            components.clone(),
            &owner,
            AgentMode::Durable,
            component_id,
            owner_kind,
            None,
            revision,
            ClaimVisibility::Hidden,
        )
        .await
        .unwrap();
        let PublicOplogEntry::SuccessfulUpdate(updated) = updated else {
            panic!("expected update")
        };
        assert_eq!(
            updated.new_active_plugins.len(),
            usize::from(owner_kind == OwnerKind::EphemeralExternalTool)
        );
        assert_eq!(
            updated.bootstrap,
            Some(OplogRegion {
                start: OplogIndex::from_u64(3),
                end: OplogIndex::from_u64(7),
            })
        );

        let activated = PublicOplogEntry::from_oplog_entry(
            OplogIndex::INITIAL,
            OplogEntry::ActivatePlugin {
                timestamp,
                plugin_grant_id: grant_id,
            },
            oplog_service.clone(),
            components.clone(),
            &owner,
            AgentMode::Durable,
            component_id,
            owner_kind,
            None,
            revision,
            ClaimVisibility::Hidden,
        )
        .await;
        assert_eq!(
            activated.is_ok(),
            owner_kind == OwnerKind::EphemeralExternalTool
        );
    }

    let projection = CreateProjectionParameters {
        agent_id: AgentId::from_parts(0, 1),
        agent_type_name: None,
        owner_kind: OwnerKind::EphemeralExternalTool,
        agent_mode: AgentMode::Ephemeral,
        component_revision: revision,
        env: vec![("observed".to_string(), "value".to_string())],
        environment_id: EnvironmentId::new(),
        created_by: AccountId::new(),
        parent: Some(owner.agent_id),
        component_size: 73,
        initial_total_linear_memory_size: 41,
        initial_active_plugins: [grant_id].into_iter().collect(),
        local_agent_config: Vec::new(),
        instance_id: Uuid::new_v4(),
        never_live: true,
    };
    let public = PublicOplogEntry::from_enrichment_entry(
        OplogIndex::INITIAL,
        OplogEnrichmentEntry::CreateProjection {
            timestamp,
            parameters: Box::new(projection.clone()),
        },
        oplog_service,
        components,
        &owner,
        AgentMode::Durable,
        component_id,
        OwnerKind::OplogProcessor,
        None,
        ComponentRevision::new(9).unwrap(),
        ClaimVisibility::Hidden,
    )
    .await
    .unwrap();
    let PublicOplogEntry::Create(public) = public else {
        panic!("expected create")
    };
    assert_eq!(public.timestamp, timestamp);
    assert_eq!(public.agent_id, projection.agent_id);
    assert_eq!(public.owner_kind, projection.owner_kind);
    assert_eq!(public.agent_mode, projection.agent_mode);
    assert_eq!(public.component_revision, projection.component_revision);
    assert_eq!(public.environment_id, projection.environment_id);
    assert_eq!(public.created_by, projection.created_by);
    assert_eq!(public.parent, projection.parent);
    assert_eq!(public.env, projection.env.into_iter().collect());
    assert_eq!(public.component_size, projection.component_size);
    assert_eq!(
        public.initial_total_linear_memory_size,
        projection.initial_total_linear_memory_size
    );
    assert_eq!(public.instance_id, projection.instance_id);
    assert!(public.never_live);
    assert_eq!(public.initial_active_plugins.len(), 1);
    assert_eq!(
        public.initial_active_plugins.first().unwrap().plugin_name,
        "component-plugin"
    );
    let wire = crate::preview2::golem_api_1_x::oplog::PublicOplogEntry::try_from(
        PublicOplogEntry::Create(CreateParams {
            owner_kind: OwnerKind::OplogProcessor,
            ..public
        }),
    )
    .unwrap();
    let crate::preview2::golem_api_1_x::oplog::PublicOplogEntry::Create(wire) = wire else {
        panic!("expected wire create")
    };
    assert!(matches!(
        wire.owner_kind,
        crate::preview2::golem_api_1_x::oplog::OwnerKind::OplogProcessor
    ));
    assert!(wire.never_live);
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
                shell: false,
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
        caller: crate::test_callers::agent_caller(owner.agent_id),
        plan: EntityInvocationPlanReference::Root { plan },
        authority_wallet: Vec::new(),
    };
    HostRequestEntityInvocation {
        metadata: desert_rust::serialize_to_byte_vec(&metadata).unwrap(),
        input,
        stream_session_idempotency_key: golem_common::model::InvocationId(uuid::Uuid::from_u128(
            601,
        )),
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
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId::from_parts(7, 2);
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
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        ComponentRevision::INITIAL,
        OplogIndex::NONE,
        1,
        ClaimVisibility::Hidden,
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
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId::from_parts(7, 3);
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
            driver: None,
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
            driver: None,
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
    let agent_span_closed = oplog
        .add(span_closed(None, observational_owner))
        .await
        .unwrap();
    let child_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(middleware_start),
            function_name: HostFunctionName::Custom("entity-child".to_string()),
            invocation_id: None,
            observational_owner: None,
            driver: None,
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
            resolved_at: jiff::Timestamp::new(1_700_000_000, 0).unwrap(),
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
            driver: None,
            request: Some(OplogPayload::Inline(Box::new(tool_request))),
            durable_function_type: DurableFunctionType::WriteLocal,
            span_started: Some(Box::new(SpanStarted {
                span_id: SpanId::generate(),
                trace_id: golem_common::model::invocation_context::TraceId::generate(),
                trace_states: Vec::new(),
                parent_span_id: None,
                links: Vec::new(),
                started_at: Timestamp::now_utc(),
                attributes: AttributeMap(BTreeMap::new()),
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
            Some(uuid::Uuid::from_u128(602).into()),
            golem_common::model::oplog::AgentError::TransientError("entity retry".to_string()),
            agent_entry,
            None,
            None,
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
    let entity_span_closed = oplog
        .add(span_closed(Some(tool_start), tool_start))
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
            calling_agent: AgentId::from_parts(7, 4),
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
            driver: None,
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
            driver: None,
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
            driver: None,
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
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        ComponentRevision::INITIAL,
        OplogIndex::INITIAL,
        final_log.as_u64() as usize,
        ClaimVisibility::Hidden,
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
        agent_span_closed,
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
        entity_span_closed,
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

    let PublicOplogEntry::SpanClosed(span_closed_params) =
        &chunk.entries[entity_span_closed.as_u64() as usize - 1].entry
    else {
        panic!("expected SpanClosed");
    };
    assert_eq!(span_closed_params.resource_start_index, Some(tool_start));
    assert_eq!(
        span_closed_params.span_finished.outcome,
        PublicSpanOutcome::Completed
    );

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
        AgentId::from_parts(7, 4).to_string(),
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
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        ComponentRevision::INITIAL,
        log_index,
        1,
        ClaimVisibility::Hidden,
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
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        ComponentRevision::INITIAL,
        OplogIndex::INITIAL,
        None,
        1,
        "last-entity-attribution-needle",
        ScanBudget::default(),
        ClaimVisibility::Hidden,
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
async fn explicit_entity_attribution_rejects_non_start_and_non_entity_anchors() {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            blob_storage,
            1,
            1,
            100,
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId::from_parts(7, 5);
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
            driver: None,
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
            driver: None,
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
    oplog.commit(CommitLevel::Always).await.unwrap();

    let components: Arc<dyn ComponentService> = Arc::new(PanicComponentService);
    let valid_chunk = get_public_oplog_chunk(
        components.clone(),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        ComponentRevision::INITIAL,
        valid,
        1,
        ClaimVisibility::Hidden,
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
    ] {
        let result = get_public_oplog_chunk(
            components.clone(),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            index,
            1,
            ClaimVisibility::Hidden,
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
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId::from_parts(7, 6);
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
            HostResponseP3SocketsUdpSend {
                result: Ok(()),
                endpoint: None,
            }
            .into(),
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
                endpoint: None,
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
            HostFunctionName::P3SocketsTypesTcpSocketSendChunk,
            HostRequestNoInput {}.into(),
            HostResponseP3SocketsTcpSendChunk { result: Ok(4096) }.into(),
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
        stream_session_idempotency_key: golem_common::model::InvocationId(uuid::Uuid::from_u128(
            601,
        )),
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
            driver: None,
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
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::new(1).unwrap(),
            ClaimVisibility::Hidden,
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
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = crate::test_agent_id(name);
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

#[test]
async fn pending_and_started_methods_expose_original_input_without_component_metadata() {
    use golem_common::model::agent::{AgentMethodInput, JsonMethodInput, PublicMethodInput};
    use golem_common::model::caller::Caller;
    use golem_common::model::card::{InvocationWalletPin, WalletVersionToken};
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::{AgentInvocationPayload, InvocationId};
    use golem_common::schema::FromSchema;

    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("original-input").await;
    let key = InvocationId(Uuid::new_v4());
    let context = InvocationContextStack::fresh();
    let input = JsonMethodInput::capture(br#"{"rejected":1e400,"signedZero":-0.0}"#).unwrap();
    let (operation, payload) = AgentInvocationPayload::AgentMethod {
        method_name: "removed_method".into(),
        allow_streams: false,
        input: AgentMethodInput::PublicJson(input.clone()),
        caller: Caller::anonymous(),
        scope_card: None,
    }
    .into_stored();
    let payload = oplog.upload_payload(&payload).await.unwrap();
    let pending = OplogEntry::pending_agent_invocation(
        None,
        key,
        Box::new(operation.clone()),
        payload.clone(),
        context.trace_id.clone(),
        context.trace_states.clone(),
        context.to_oplog_data(),
    );
    let started = OplogEntry::AgentInvocationStarted {
        timestamp: Timestamp::now_utc(),
        invocation_id: key,
        operation: Box::new(operation),
        payload,
        trace_id: context.trace_id.clone(),
        trace_states: context.trace_states.clone(),
        invocation_context: context.to_oplog_data(),
        wallet_pin: Box::new(InvocationWalletPin {
            wallet_token: WalletVersionToken {
                wallet_id_hash: [0; 32],
                generation: 0,
            },
            pinned_card_ids: Vec::new(),
            scope_card_id: None,
        }),
    };
    for entry in [pending, started] {
        let public = PublicOplogEntry::from_oplog_entry(
            OplogIndex::INITIAL,
            entry,
            oplog_service.clone(),
            Arc::new(PanicComponentService),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            ClaimVisibility::Hidden,
        )
        .await
        .unwrap();
        let invocation = match public {
            PublicOplogEntry::PendingAgentInvocation(params) => params.invocation,
            PublicOplogEntry::AgentInvocationStarted(params) => params.invocation,
            _ => panic!("expected a method invocation"),
        };
        let PublicAgentInvocation::AgentMethodInvocation(method) = invocation else {
            panic!("expected original method input")
        };
        let PublicMethodInput::PublicJson(raw) =
            PublicMethodInput::from_value(method.function_input.value()).unwrap()
        else {
            panic!("expected captured JSON")
        };
        assert_eq!(raw, input.as_str());
    }
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
            bootstrap: Some(OplogRegion {
                start: OplogIndex::from_u64(3),
                end: OplogIndex::from_u64(7),
            }),
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
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        page_revision,
        OplogIndex::INITIAL,
        page_limits(1_000_000, 1000),
        ClaimVisibility::Hidden,
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
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            omitted.oplog_index,
            Some(omitted.component_revision),
            ClaimVisibility::Hidden,
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
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        snapshot_index,
        None,
        ClaimVisibility::Hidden,
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
    assert_eq!(
        snapshot.bootstrap,
        Some(OplogRegion {
            start: OplogIndex::from_u64(3),
            end: OplogIndex::from_u64(7),
        })
    );

    assert!(
        get_public_oplog_entry(
            Arc::new(PanicComponentService),
            oplog_service,
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            large_log_index.next(),
            None,
            ClaimVisibility::Hidden,
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
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            from,
            page_limits(2500, 1_000_000),
            ClaimVisibility::Hidden,
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
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            from,
            PublicOplogPageLimits {
                count: 3,
                max_bytes: 2500,
                max_entry_bytes: 1_000_000,
            },
            "needle",
            ScanBudget::default(),
            ClaimVisibility::Hidden,
        )
    };

    // The 2000 byte log entries do not match and do not count against the budget, so the first
    // two matches fit, from different chunks, and the third match ends the page.
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
async fn an_oplog_search_returns_at_most_count_matches_and_resumes_after_the_last() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("search-count").await;
    let needle = || OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        level: LogLevel::Stderr,
        context: "context".to_string(),
        message: "needle".to_string(),
        trace_context: None,
    };
    let first_index = oplog.add(log_entry(0)).await.unwrap();
    let mut matching = Vec::new();
    for _ in 0..4 {
        matching.push(oplog.add(needle()).await.unwrap());
    }
    oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(matching[0], first_index.next());

    // A first chunk of two entries finds one match. The search then reads only the one entry
    // that it still needs, not a full chunk, which would find two more matches.
    let page = |from: OplogIndex| {
        search_public_oplog_page(
            Arc::new(PanicComponentService),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            from,
            PublicOplogPageLimits {
                count: 2,
                max_bytes: 1_000_000,
                max_entry_bytes: 1_000_000,
            },
            "needle",
            ScanBudget::default(),
            ClaimVisibility::Hidden,
        )
    };
    let first = page(OplogIndex::INITIAL).await.unwrap();
    assert_eq!(
        page_indices(&first.entries),
        vec![(matching[0], true), (matching[1], true)]
    );
    assert_eq!(first.next_oplog_index, matching[2]);
    let second = page(first.next_oplog_index).await.unwrap();
    assert_eq!(
        page_indices(&second.entries),
        vec![(matching[2], true), (matching[3], true)]
    );

    // A guest search has the same limit, and still reads no entry after its pinned end.
    let guest = |from: OplogIndex| {
        search_public_oplog(
            Arc::new(PanicComponentService),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            from,
            Some(matching[2]),
            2,
            "needle",
            ScanBudget::default(),
            ClaimVisibility::Hidden,
        )
    };
    let first = guest(OplogIndex::INITIAL).await.unwrap();
    let indices = |entries: &[PublicOplogEntryWithIndex]| {
        entries
            .iter()
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>()
    };
    assert_eq!(indices(&first.entries), vec![matching[0], matching[1]]);
    assert_eq!(first.next_oplog_index, matching[2]);
    let second = guest(first.next_oplog_index).await.unwrap();
    assert_eq!(indices(&second.entries), vec![matching[2]]);
    assert_eq!(second.last_index, matching[2]);
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
                bootstrap: None,
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

#[test]
async fn a_cursor_carries_the_next_index_and_the_revision_and_a_stale_cursor_is_refused() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("cursor-rule").await;
    let updated_revision = ComponentRevision::new(7).unwrap();
    let mut indices = Vec::new();
    for i in 0..13 {
        let entry = if i == 1 {
            OplogEntry::SuccessfulUpdate {
                timestamp: Timestamp::now_utc(),
                target_revision: updated_revision,
                new_component_size: 100,
                new_total_linear_memory_size: None,
                new_active_plugins: HashSet::new(),
                snapshot_assisted_details: None,
                bootstrap: None,
            }
        } else if i == 9 {
            OplogEntry::Log {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                level: LogLevel::Stderr,
                context: "context".to_string(),
                message: "needle".to_string(),
                trace_context: None,
            }
        } else {
            log_entry(0)
        };
        indices.push(oplog.add(entry).await.unwrap());
    }
    oplog.commit(CommitLevel::Always).await.unwrap();
    let last_index = *indices.last().unwrap();

    let budget = ScanBudget {
        entries: std::num::NonZeroU64::new(5).unwrap(),
        time: std::time::Duration::from_secs(600),
    };
    let search = |start: OplogIndex, revision: ComponentRevision| {
        search_public_oplog_page(
            Arc::new(PanicComponentService),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            revision,
            start,
            PublicOplogPageLimits {
                count: 3,
                max_bytes: 1_000_000,
                max_entry_bytes: 1_000_000,
            },
            "needle",
            budget,
            ClaimVisibility::Hidden,
        )
    };

    // A read without a cursor starts after the update, with the revision that the update set.
    let (start, revision) = oplog_page_start(
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        None,
        indices[2],
    )
    .await
    .unwrap();
    assert_eq!((start, revision), (indices[2], updated_revision));

    // The budget ends the first search after five entries, before the match. The page is empty,
    // and its cursor points to the first entry it did not read, with the revision and the hash of
    // the query.
    let first = search(start, revision).await.unwrap();
    assert!(first.entries.is_empty());
    let cursor = oplog_page_cursor(
        first.next_oplog_index,
        first.current_component_revision,
        first.last_index,
        Some("needle"),
    )
    .expect("a page that ends before the last entry has a cursor");
    assert_eq!(
        cursor,
        OplogCursor {
            next_oplog_index: indices[7].as_u64(),
            current_component_revision: updated_revision.into(),
            query_hash: OplogCursor::query_hash(Some("needle")),
        }
    );

    // A read of another query, or a read without a query, refuses the cursor.
    for query in [Some("needles"), Some(""), None] {
        assert_eq!(
            check_oplog_cursor_query(&owned_agent_id, Some(&cursor), query),
            Err(WorkerExecutorError::StaleCursor {
                agent_id: owned_agent_id.agent_id,
                next_oplog_index: indices[7],
                reason: StaleCursorReason::QueryMismatch,
            }),
            "a read of {query:?} must refuse the cursor of a search for \"needle\""
        );
    }
    assert_eq!(
        check_oplog_cursor_query(&owned_agent_id, Some(&cursor), Some("needle")),
        Ok(())
    );

    // The cursor continues where the first search stopped, with the revision it carries.
    let (start, revision) = oplog_page_start(
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        Some(cursor),
        OplogIndex::INITIAL,
    )
    .await
    .unwrap();
    assert_eq!((start, revision), (indices[7], updated_revision));
    let second = search(start, revision).await.unwrap();
    assert_eq!(page_indices(&second.entries), vec![(indices[9], true)]);
    let second_cursor = oplog_page_cursor(
        second.next_oplog_index,
        second.current_component_revision,
        second.last_index,
        Some("needle"),
    )
    .expect("the budget ends the second search before the last entry");
    assert_eq!(second_cursor.next_oplog_index, indices[12].as_u64());
    assert_eq!(
        second_cursor.current_component_revision,
        u64::from(updated_revision)
    );

    // A page that reads the last entry has no cursor.
    let third = search(indices[12], updated_revision).await.unwrap();
    assert!(third.entries.is_empty());
    assert_eq!(
        oplog_page_cursor(
            third.next_oplog_index,
            third.current_component_revision,
            third.last_index,
            Some("needle"),
        ),
        None
    );

    // A cursor at the last entry is valid; one past it never came from a page of this oplog.
    for (next_oplog_index, accepted) in [
        (last_index.as_u64(), true),
        (last_index.as_u64() + 1, false),
        (last_index.as_u64() + 1000, false),
    ] {
        let result = oplog_page_start(
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            Some(OplogCursor {
                next_oplog_index,
                current_component_revision: updated_revision.into(),
                query_hash: OplogCursor::query_hash(Some("needle")),
            }),
            OplogIndex::INITIAL,
        )
        .await;
        match result {
            Ok((start, _)) => {
                assert!(accepted, "a cursor at {next_oplog_index} must be refused");
                assert_eq!(start.as_u64(), next_oplog_index);
            }
            Err(error) => {
                assert!(!accepted, "a cursor at {next_oplog_index} must be accepted");
                assert_eq!(
                    error,
                    WorkerExecutorError::StaleCursor {
                        agent_id: owned_agent_id.agent_id,
                        next_oplog_index: OplogIndex::from_u64(next_oplog_index),
                        reason: StaleCursorReason::PastEnd {
                            last_oplog_index: last_index,
                        },
                    }
                );
            }
        }
    }
}

/// Component service stub that serves the metadata of one component at every revision, for
/// entries whose rendering reads the agent type.
struct MetadataComponentService(Component);

#[async_trait]
impl ComponentService for MetadataComponentService {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        panic!("rendering must not load executable components")
    }

    async fn get_metadata(
        &self,
        _component_id: ComponentId,
        _forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        Ok(self.0.clone())
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: golem_common::model::application::ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        panic!("rendering must use the supplied component identity")
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        Vec::new()
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
}

fn metadata_component_service(owned_agent_id: &OwnedAgentId) -> Arc<dyn ComponentService> {
    Arc::new(MetadataComponentService(Component {
        id: test_component_id(),
        revision: ComponentRevision::INITIAL,
        environment_id: owned_agent_id.environment_id,
        component_name: ComponentName("callers".to_string()),
        hash: golem_common::model::diff::Hash::empty(),
        application_id: golem_common::model::application::ApplicationId::new(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("test@golem"),
        application_name: "test-app".parse().unwrap(),
        environment_name: "test-env".parse().unwrap(),
        component_size: 100,
        metadata: golem_common::model::component_metadata::ComponentMetadata::from_parts(
            Default::default(),
            Vec::new(),
            Vec::new(),
            BTreeMap::new(),
        ),
        created_at: jiff::Timestamp::now(),
        wasm_hash: golem_common::model::diff::Hash::empty(),
        package_metadata: None,
        object_store_key: String::new(),
    }))
}

fn external_tool_activation(
    tool_name: &ToolName,
) -> golem_common::model::tool::ToolActivationSnapshot {
    let source = ToolSource::Component {
        component_id: Default::default(),
        component_revision: ComponentRevision::INITIAL,
        component_name: "test:tool".try_into().unwrap(),
    };
    golem_common::model::tool::ToolActivationSnapshot {
        registered_tool: golem_common::model::tool::ToolRegistration {
            deployment_revision: DeploymentRevision::INITIAL,
            release_id: None,
            definition: test_tool_definition(),
            provision: Default::default(),
            source: source.clone(),
            owner_account_id: Default::default(),
            owner_account_email: "owner@example.com".into(),
            metadata_version: String::new(),
            metadata_digest: Default::default(),
            component_bindings: Default::default(),
        },
        binding: CompiledToolBinding {
            deployment_revision: DeploymentRevision::INITIAL,
            release_id: None,
            owner: golem_common::model::tool::ToolBindingOwner::AgentType {
                agent_type_name: "Test".parse().unwrap(),
            },
            tool_name: tool_name.clone(),
            version: "1.0.0".to_string(),
            metadata_version: String::new(),
            metadata_digest: Default::default(),
            account_id: Default::default(),
            account_email: "owner@example.com".into(),
            parameters: NormalizedJsonValue::new(serde_json::Value::Null),
            config_keys_readable: Default::default(),
            secret_keys_readable: Default::default(),
            secret_keys_revealable: Default::default(),
            filesystem_access: ToolFilesystemAccess::Denied,
            shell: false,
            source,
        },
        filesystem: FilesystemCapability::Incapable,
        middleware_chain: None,
        mcp_import: None,
    }
}

/// The recorded invocation kinds that carry a caller, each with `caller`.
fn invocation_payloads_with_caller(
    owned_agent_id: &OwnedAgentId,
    caller: &golem_common::model::caller::Caller,
) -> Vec<AgentInvocationPayload> {
    let tool_name = ToolName::try_from("grep").unwrap();
    vec![
        AgentInvocationPayload::AgentMethod {
            method_name: "run".to_string(),
            input: SchemaValue::Record { fields: Vec::new() }
                .try_into()
                .unwrap(),
            allow_streams: false,
            caller: caller.clone(),
            scope_card: None,
        },
        AgentInvocationPayload::ExternalTool {
            tool_name: tool_name.clone(),
            command_path: vec!["search".to_string()],
            input: Box::new(TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
                SchemaValue::Tuple {
                    elements: Vec::new(),
                },
            )),
            stdin: false,
            stdout: false,
            stderr: false,
            activation: Box::new(external_tool_activation(&tool_name)),
            caller: caller.clone(),
            scope_card: None,
        },
        AgentInvocationPayload::ProcessOplogEntries {
            account_id: AccountId::new(),
            config: Vec::new(),
            metadata: golem_common::model::oplog::payload::types::AgentMetadataForGuests {
                agent_id: owned_agent_id.agent_id,
                agent_type_name: None,
                fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::nil()),
                args: Vec::new(),
                env: Vec::new(),
                config: BTreeMap::new(),
                status: golem_common::model::AgentStatus::Running,
                component_revision: ComponentRevision::INITIAL,
                retry_count: 0,
                environment_id: owned_agent_id.environment_id,
            },
            first_entry_index: OplogIndex::INITIAL,
            entries: Vec::new(),
            caller: caller.clone(),
        },
        AgentInvocationPayload::SaveSnapshot {
            caller: caller.clone(),
        },
    ]
}

/// Adds a started and a pending invocation entry for each payload, and returns the indexes of the
/// started entries.
async fn add_invocation_entries(
    oplog: &Arc<dyn crate::services::oplog::Oplog>,
    payloads: Vec<AgentInvocationPayload>,
) -> Vec<OplogIndex> {
    let mut started = Vec::new();
    for (ordinal, payload) in payloads.into_iter().enumerate() {
        let (operation, stored) = payload.into_stored();
        let bytes = golem_common::serialization::serialize(&stored).unwrap();
        let index = oplog
            .add(OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::from(1_000),
                invocation_id: Uuid::from_u128(0x100 + ordinal as u128).into(),
                operation: Box::new(operation.clone()),
                payload: OplogPayload::SerializedInline {
                    bytes: bytes.clone(),
                    cached: None,
                },
                trace_id: golem_common::model::invocation_context::TraceId::generate(),
                trace_states: Vec::new(),
                invocation_context: Vec::new(),
                wallet_pin: Box::new(golem_common::model::card::InvocationWalletPin {
                    wallet_token: golem_common::model::card::WalletVersionToken {
                        wallet_id_hash: [0; 32],
                        generation: 0,
                    },
                    pinned_card_ids: Vec::new(),
                    scope_card_id: None,
                }),
            })
            .await
            .unwrap();
        started.push(index);
        oplog
            .add(OplogEntry::PendingAgentInvocation {
                timestamp: Timestamp::from(1_000),
                not_before: None,
                invocation_id: Uuid::from_u128(0x200 + ordinal as u128).into(),
                operation: Box::new(operation),
                payload: OplogPayload::SerializedInline {
                    bytes,
                    cached: None,
                },
                trace_id: golem_common::model::invocation_context::TraceId::generate(),
                trace_states: Vec::new(),
                invocation_context: Vec::new(),
            })
            .await
            .unwrap();
    }
    oplog.commit(CommitLevel::Always).await.unwrap();
    started
}

fn public_invocation(entry: &PublicOplogEntry) -> &PublicAgentInvocation {
    match entry {
        PublicOplogEntry::AgentInvocationStarted(params) => &params.invocation,
        PublicOplogEntry::PendingAgentInvocation(params) => &params.invocation,
        other => panic!("expected an invocation entry, got {other:?}"),
    }
}

#[test]
async fn invocation_callers_render_through_public_oplog_api_and_wit() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("invocation-callers").await;
    let caller = crate::test_callers::sample_caller();
    let payloads = invocation_payloads_with_caller(&owned_agent_id, &caller);
    let kinds = payloads.len();
    add_invocation_entries(&oplog, payloads).await;
    let components = metadata_component_service(&owned_agent_id);

    for claims in [ClaimVisibility::Hidden, ClaimVisibility::Shown] {
        let expected_caller = PublicCaller::new(&caller, claims);

        let chunk = get_public_oplog_chunk(
            components.clone(),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            OplogIndex::INITIAL,
            100,
            claims,
        )
        .await
        .unwrap();
        assert_eq!(chunk.entries.len(), 2 * kinds);
        for entry in &chunk.entries {
            assert_eq!(
                public_invocation(&entry.entry).caller(),
                Some(&expected_caller),
                "the caller of entry {}",
                entry.oplog_index
            );
        }

        // The page that the worker service reads holds the same callers, and the claims only
        // when they were asked for.
        let page = get_public_oplog_page(
            components.clone(),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            OplogIndex::INITIAL,
            PublicOplogPageLimits {
                count: 100,
                max_bytes: 1_000_000,
                max_entry_bytes: 1_000_000,
            },
            claims,
        )
        .await
        .unwrap();
        let encoded = page
            .entries
            .iter()
            .flat_map(|entry| entry.encode_to_vec())
            .collect::<Vec<u8>>();
        let holds_claims = encoded.windows(6).any(|window| window == b"\"deep\"");
        assert_eq!(holds_claims, claims == ClaimVisibility::Shown);
        let page_entries = public_page_entries(&page.entries);
        assert_eq!(page_entries.len(), 2 * kinds);
        for (page_entry, entry) in page_entries.iter().zip(&chunk.entries) {
            let PublicOplogPageEntry::Included(page_entry) = page_entry else {
                panic!("expected an included entry at {}", entry.oplog_index);
            };
            // An invocation without a recorded context gets a fresh root span on each read, so
            // the entries are compared by their index and their caller.
            assert_eq!(page_entry.oplog_index, entry.oplog_index);
            assert_eq!(
                public_invocation(&page_entry.entry).caller(),
                public_invocation(&entry.entry).caller()
            );
        }

        // Guests get both roles and the tokens, never the claims.
        for entry in chunk.entries {
            let wit_entry =
                crate::preview2::golem_api_1_x::oplog::PublicOplogEntry::try_from(entry.entry)
                    .unwrap();
            use crate::preview2::golem_api_1_x::oplog;
            let invocation = match wit_entry {
                oplog::PublicOplogEntry::AgentInvocationStarted(params) => params.invocation,
                oplog::PublicOplogEntry::PendingAgentInvocation(params) => params.invocation,
                other => panic!("expected an invocation entry, got {other:?}"),
            };
            let wit_caller = match invocation {
                oplog::AgentInvocation::AgentMethodInvocation(params) => params.caller,
                oplog::AgentInvocation::ExternalTool(params) => params.caller,
                oplog::AgentInvocation::ProcessOplogEntries(params) => params.caller,
                oplog::AgentInvocation::SaveSnapshot(params) => params.caller,
                other => panic!("unexpected invocation kind {other:?}"),
            };
            let oplog::PublicPrincipal::User(user) = wit_caller.original else {
                panic!("expected a user at the root");
            };
            assert_eq!(user.pair.subject, "user-1");
            let oplog::PublicPrincipal::Agent(agent) = wit_caller.immediate else {
                panic!("expected an agent as the immediate caller");
            };
            assert_eq!(agent.agent_type, "TestAgent");
            assert_eq!(wit_caller.tokens.len(), 2);
        }
    }
}

/// A user whose claims hold `size` bytes of padding.
fn user_with_claims_of_size(size: usize) -> golem_common::model::caller::Principal {
    golem_common::model::caller::Principal::User(golem_common::model::caller::UserPrincipal {
        pair: golem_common::model::caller::Pair::new("http://test-idp.com", "bob"),
        claims: golem_common::model::caller::Claims::from_json(format!(
            r#"{{"sub":"bob","padding":"{}"}}"#,
            "x".repeat(size)
        ))
        .unwrap(),
        authenticated_at: Timestamp::from(1_000),
    })
}

#[test]
async fn hidden_claims_do_not_make_an_oplog_entry_omitted() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("large-claims").await;
    let caller = golem_common::model::caller::Caller::entry(user_with_claims_of_size(512 * 1024));
    let payloads = invocation_payloads_with_caller(&owned_agent_id, &caller);
    let started_index =
        add_invocation_entries(&oplog, payloads.into_iter().take(1).collect()).await[0];
    let components = metadata_component_service(&owned_agent_id);

    let page = |claims| {
        get_public_oplog_page(
            components.clone(),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            started_index,
            PublicOplogPageLimits {
                count: 1,
                max_bytes: 8 * 1024 * 1024,
                max_entry_bytes: 64 * 1024,
            },
            claims,
        )
    };

    let hidden = page(ClaimVisibility::Hidden).await.unwrap();
    assert_eq!(page_indices(&hidden.entries), vec![(started_index, true)]);

    let shown = page(ClaimVisibility::Shown).await.unwrap();
    assert_eq!(page_indices(&shown.entries), vec![(started_index, false)]);
    let PublicOplogPageEntry::Omitted(omitted) = &public_page_entries(&shown.entries)[0] else {
        unreachable!()
    };
    assert!(omitted.size > 512 * 1024);

    let full = get_public_oplog_entry(
        components.clone(),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        started_index,
        Some(omitted.component_revision),
        ClaimVisibility::Shown,
    )
    .await
    .unwrap();
    assert_eq!(
        public_invocation(&full.entry).caller(),
        Some(&PublicCaller::new(&caller, ClaimVisibility::Shown))
    );
}

/// A pending update and a queued manual update show their requester in both roles with its
/// tokens, with claims only when they are shown. The requester is not the caller of the queued
/// manual update. Guests get both roles through the WIT, never the claims.
#[test]
async fn pending_update_public_form_shows_the_requester() {
    use crate::preview2::golem_api_1_x::oplog;

    let (oplog_service, owned_agent_id, raw_oplog) = open_test_oplog("update-requester").await;
    let requester = crate::test_callers::sample_caller();
    let target_revision = ComponentRevision::new(2).unwrap();
    let admission_index = raw_oplog
        .add(OplogEntry::PendingUpdate {
            timestamp: Timestamp::from(1_000),
            description: UpdateDescription::Automatic { target_revision },
            update_attempt_index: None,
            requester: Box::new(requester.clone()),
        })
        .await
        .unwrap();
    let (operation, stored) = AgentInvocationPayload::ManualUpdate {
        target_revision,
        requester: requester.clone(),
    }
    .into_stored();
    raw_oplog
        .add(OplogEntry::PendingAgentInvocation {
            timestamp: Timestamp::from(1_000),
            not_before: None,
            invocation_id: Uuid::from_u128(0x300).into(),
            operation: Box::new(operation),
            payload: OplogPayload::SerializedInline {
                bytes: golem_common::serialization::serialize(&stored).unwrap(),
                cached: None,
            },
            trace_id: golem_common::model::invocation_context::TraceId::generate(),
            trace_states: Vec::new(),
            invocation_context: Vec::new(),
        })
        .await
        .unwrap();
    raw_oplog.commit(CommitLevel::Always).await.unwrap();
    let components = metadata_component_service(&owned_agent_id);

    for claims in [ClaimVisibility::Hidden, ClaimVisibility::Shown] {
        let expected = PublicCaller::new(&requester, claims);
        assert_eq!(expected.original.claims().is_some(), claims.is_shown());

        let chunk = get_public_oplog_chunk(
            components.clone(),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            ComponentRevision::INITIAL,
            OplogIndex::INITIAL,
            100,
            claims,
        )
        .await
        .unwrap();
        let mut seen = Vec::new();
        for entry in &chunk.entries {
            let wit_requester = match &entry.entry {
                PublicOplogEntry::PendingUpdate(params) => {
                    assert_eq!(params.requester, expected);
                    assert_eq!(params.update_attempt_index, admission_index);
                    seen.push("pending update");
                    let oplog::PublicOplogEntry::PendingUpdate(wit) =
                        oplog::PublicOplogEntry::try_from(entry.entry.clone()).unwrap()
                    else {
                        panic!("expected a WIT pending update");
                    };
                    wit.requester
                }
                PublicOplogEntry::PendingAgentInvocation(params) => {
                    let PublicAgentInvocation::ManualUpdate(manual) = &params.invocation else {
                        panic!("expected a manual update, got {:?}", params.invocation);
                    };
                    assert_eq!(manual.requester, expected);
                    assert_eq!(params.invocation.caller(), None);
                    seen.push("queued manual update");
                    let oplog::PublicOplogEntry::PendingAgentInvocation(wit) =
                        oplog::PublicOplogEntry::try_from(entry.entry.clone()).unwrap()
                    else {
                        panic!("expected a WIT pending invocation");
                    };
                    let oplog::AgentInvocation::ManualUpdate(wit) = wit.invocation else {
                        panic!("expected a WIT manual update");
                    };
                    wit.requester
                }
                _ => continue,
            };
            let oplog::PublicPrincipal::User(user) = wit_requester.original else {
                panic!("expected a user at the root of the requester");
            };
            assert_eq!(user.pair.subject, "user-1");
            let oplog::PublicPrincipal::Agent(agent) = wit_requester.immediate else {
                panic!("expected an agent as the immediate requester");
            };
            assert_eq!(agent.agent_type, "TestAgent");
            assert_eq!(wit_requester.tokens.len(), 2);
        }
        assert_eq!(seen, vec!["pending update", "queued manual update"]);
    }
}

/// A range read shows the claims of a caller exactly when the reader asks for them, as a page
/// read does.
#[test]
async fn an_oplog_range_read_shows_claims_only_on_request() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("range-claims").await;
    let caller = golem_common::model::caller::Caller::entry(user_with_claims_of_size(16));
    let payloads = invocation_payloads_with_caller(&owned_agent_id, &caller);
    let started_index =
        add_invocation_entries(&oplog, payloads.into_iter().take(1).collect()).await[0];
    let components = metadata_component_service(&owned_agent_id);

    for claims in [ClaimVisibility::Hidden, ClaimVisibility::Shown] {
        let page = get_public_oplog_range_page(
            components.clone(),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            None,
            started_index,
            started_index,
            PublicOplogPageLimits {
                count: 1,
                max_bytes: 1_000_000,
                max_entry_bytes: 1_000_000,
            },
            OplogScanAllowance {
                entries: 100,
                millis: 600_000,
            },
            claims,
        )
        .await
        .unwrap();
        let entries = public_page_entries(&page.entries);
        let PublicOplogPageEntry::Included(entry) = &entries[0] else {
            panic!("the range read leaves out the entry: {entries:?}");
        };
        assert_eq!(
            public_invocation(&entry.entry).caller(),
            Some(&PublicCaller::new(&caller, claims)),
            "{claims:?}"
        );
    }
}

/// The byte strings in the JSON form of a public entry: lists of `u8` values and binary payloads.
fn byte_strings(value: &serde_json::Value, out: &mut Vec<Vec<u8>>) {
    match value {
        serde_json::Value::Array(items) => {
            let as_u8 = |item: &serde_json::Value| {
                let number = match item {
                    serde_json::Value::Object(map) if map.get("kind") == Some(&"u8".into()) => {
                        map.get("value")
                    }
                    other => Some(other),
                };
                number
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| u8::try_from(n).ok())
            };
            match items.iter().map(as_u8).collect::<Option<Vec<u8>>>() {
                Some(bytes) if !bytes.is_empty() => out.push(bytes),
                _ => items.iter().for_each(|item| byte_strings(item, out)),
            }
        }
        serde_json::Value::Object(map) => map.values().for_each(|item| byte_strings(item, out)),
        _ => {}
    }
}

/// An oplog processor passes the raw entries of its batch to `enrich-oplog-entries`, and the
/// executor records that request in the processor's own oplog. The raw entries hold the claims of
/// the users who called the processed agent, so the public form of the `Start` entry, in REST and
/// in the WIT `get-oplog`, keeps only their indexes.
#[test]
async fn the_public_enrich_request_holds_no_raw_entries() {
    const MARKER: &str = "claimsmarker5e2b";
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("processor-agent").await;

    // The raw `agent-invocation-started` of the processed agent, as the processor's batch holds it.
    let bob = golem_common::model::caller::Caller::entry(
        golem_common::model::caller::Principal::User(golem_common::model::caller::UserPrincipal {
            pair: golem_common::model::caller::Pair::new("http://test-idp.com", "bob"),
            claims: golem_common::model::caller::Claims::from_json(format!(
                r#"{{"sub":"bob","email":"{MARKER}@mail.example"}}"#
            ))
            .unwrap(),
            authenticated_at: Timestamp::from(1_000),
        }),
    );
    let (operation, stored) = AgentInvocationPayload::AgentMethod {
        method_name: "run".to_string(),
        input: SchemaValue::Record { fields: Vec::new() }
            .try_into()
            .unwrap(),
        allow_streams: false,
        caller: bob,
        scope_card: None,
    }
    .into_stored();
    let raw_started = OplogEntry::AgentInvocationStarted {
        timestamp: Timestamp::from(1_000),
        invocation_id: Uuid::from_u128(0x100).into(),
        operation: Box::new(operation),
        payload: OplogPayload::Inline(Box::new(stored)),
        trace_id: golem_common::model::invocation_context::TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
        wallet_pin: Box::new(golem_common::model::card::InvocationWalletPin {
            wallet_token: golem_common::model::card::WalletVersionToken {
                wallet_id_hash: [0; 32],
                generation: 0,
            },
            pinned_card_ids: Vec::new(),
            scope_card_id: None,
        }),
    };
    // The batch entry the guest gets, and the entry the host decodes from the guest's call, as
    // `prepare_oplog_enrichment_entries` does.
    let wit_entry =
        crate::preview2::golem_api_1_x::oplog::OplogEntry::try_from(raw_started).unwrap();
    let enrichment = golem_common::model::oplog::OplogEnrichmentEntry::try_from(wit_entry).unwrap();
    let request_entry = golem_common::serialization::serialize(&enrichment).unwrap();

    // The recorded request of the processor's `enrich-oplog-entries` call.
    let request: golem_common::model::oplog::HostRequest =
        golem_common::model::oplog::HostRequestGolemApiOplogEnrich {
            environment_id: owned_agent_id.environment_id(),
            agent_id: owned_agent_id.agent_id,
            entries: vec![(5, request_entry)],
            component_revision: 0,
        }
        .into();
    let request_payload = oplog_service
        .upload_payload(&owned_agent_id, AgentMode::Durable, &request)
        .await
        .unwrap();
    let start_index = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::GolemApiEnrichOplogEntries,
            invocation_id: None,
            observational_owner: None,
            request: Some(request_payload),
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: None,
            denial: None,
            driver: None,
        })
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let raw = oplog_service
        .read_exact(
            &owned_agent_id,
            AgentMode::Durable,
            start_index,
            Into::<u64>::into(start_index),
        )
        .await
        .unwrap();
    let components: Arc<dyn ComponentService> = Arc::new(PanicComponentService);
    for (index, raw_entry) in raw {
        // The public form that a REST reader without `include=claims` gets.
        let mut public_entry = PublicOplogEntry::from_oplog_entry(
            index,
            raw_entry,
            oplog_service.clone(),
            components.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::OplogProcessor,
            None,
            ComponentRevision::INITIAL,
            ClaimVisibility::Hidden,
        )
        .await
        .unwrap();

        // The guest form that `get-oplog` gives to a guest that reads the processor's oplog.
        let wit_entry =
            crate::preview2::golem_api_1_x::oplog::PublicOplogEntry::try_from(public_entry.clone())
                .unwrap();
        let crate::preview2::golem_api_1_x::oplog::PublicOplogEntry::Start(wit_start) = wit_entry
        else {
            panic!("expected a Start entry");
        };
        let wit_request =
            golem_schema::schema::wit::decode_typed(wit_start.request.as_ref().unwrap()).unwrap();
        let mut wit_bytes = Vec::new();
        byte_strings(
            &serde_json::to_value(wit_request.value()).unwrap(),
            &mut wit_bytes,
        );
        let wit_leaked = wit_bytes
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .filter(|text| text.contains(MARKER))
            .count();

        public_entry.redact_host_managed_values_for_external();
        let json = serde_json::to_value(&public_entry).unwrap();
        let mut bytes = Vec::new();
        byte_strings(&json, &mut bytes);
        let leaked = bytes
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .filter(|text| text.contains(MARKER))
            .collect::<Vec<_>>();
        assert!(
            leaked.is_empty(),
            "the REST form of entry {index} holds the claims"
        );
        assert_eq!(
            wit_leaked, 0,
            "the WIT form of entry {index} holds the claims"
        );
        let request = &json["request"].to_string();
        assert!(request.contains('5'), "the entry index is kept: {request}");
    }
}

#[test]
async fn an_oplog_range_read_stops_at_its_end_and_accounts_its_scan() {
    let (oplog_service, owned_agent_id, oplog) = open_test_oplog("range-read").await;
    let updated_revision = ComponentRevision::new(4).unwrap();
    let mut indices = Vec::new();
    for i in 0..12 {
        let entry = if i == 2 {
            OplogEntry::SuccessfulUpdate {
                timestamp: Timestamp::now_utc(),
                target_revision: updated_revision,
                new_component_size: 100,
                new_total_linear_memory_size: None,
                new_active_plugins: HashSet::new(),
                snapshot_assisted_details: None,
                bootstrap: None,
            }
        } else {
            log_entry(0)
        };
        indices.push(oplog.add(entry).await.unwrap());
    }
    oplog.commit(CommitLevel::Always).await.unwrap();
    let allowance = |entries| OplogScanAllowance {
        entries,
        millis: 600_000,
    };
    let read = |cursor: Option<OplogCursor>, from: OplogIndex, to: OplogIndex, count, a| {
        get_public_oplog_range_page(
            Arc::new(PanicComponentService),
            oplog_service.clone(),
            &owned_agent_id,
            AgentMode::Durable,
            test_component_id(),
            OwnerKind::ComponentAgent,
            None,
            cursor,
            from,
            to,
            PublicOplogPageLimits {
                count,
                max_bytes: 1_000_000,
                max_entry_bytes: 1_000_000,
            },
            a,
            ClaimVisibility::Hidden,
        )
    };
    let (from, to) = (indices[5], indices[8]);
    // The entries before `from` count against the allowance: they hold the revision at `from`.
    let charged_before = from.as_u64() - 1;

    // A page of two, then a page that ends at `to`, though the oplog goes on.
    let first = read(None, from, to, 2, allowance(100)).await.unwrap();
    assert_eq!(
        page_indices(&first.entries),
        vec![(indices[5], true), (indices[6], true)]
    );
    assert_eq!(first.scan.charged_entries, charged_before + 2);
    assert!(!first.scan.exhausted);
    let cursor = first.next.clone().expect("the range goes on");
    assert_eq!(cursor.next_oplog_index, indices[7].as_u64());
    assert_eq!(
        cursor.current_component_revision,
        u64::from(updated_revision)
    );
    let second = read(Some(cursor), from, to, 10, allowance(100))
        .await
        .unwrap();
    assert_eq!(
        page_indices(&second.entries),
        vec![(indices[7], true), (indices[8], true)]
    );
    assert_eq!(second.next, None);
    assert_eq!(second.scan.charged_entries, 2);
    assert!(!second.scan.exhausted);
    assert!(second.last_index > to);

    // Exactly the allowance completes the range; one entry less stops before `to`.
    let exact = read(None, from, to, 10, allowance(charged_before + 4))
        .await
        .unwrap();
    assert_eq!(exact.entries.len(), 4);
    assert_eq!((exact.next, exact.scan.exhausted), (None, false));
    let short = read(None, from, to, 10, allowance(charged_before + 3))
        .await
        .unwrap();
    assert_eq!(short.entries.len(), 3);
    assert!(short.scan.exhausted);
    assert_eq!(
        short.next.map(|cursor| cursor.next_oplog_index),
        Some(to.as_u64())
    );

    // A page that its byte limit cuts short is charged with every entry that it read.
    let cut = get_public_oplog_range_page(
        Arc::new(PanicComponentService),
        oplog_service.clone(),
        &owned_agent_id,
        AgentMode::Durable,
        test_component_id(),
        OwnerKind::ComponentAgent,
        None,
        None,
        from,
        to,
        PublicOplogPageLimits {
            count: 10,
            max_bytes: 1,
            max_entry_bytes: 1_000_000,
        },
        allowance(100),
        ClaimVisibility::Hidden,
    )
    .await
    .unwrap();
    assert_eq!(page_indices(&cut.entries), vec![(indices[5], true)]);
    assert_eq!(cut.scan.charged_entries, charged_before + 4);
    assert_eq!(
        cut.next.map(|cursor| cursor.next_oplog_index),
        Some(indices[6].as_u64())
    );

    // The allowance runs out while the read looks for the revision at `from`: no page, no cursor.
    let before = read(None, from, to, 10, allowance(2)).await.unwrap();
    assert!(before.entries.is_empty());
    assert_eq!((before.next, before.scan.exhausted), (None, true));

    // A cursor outside the range, or a range that ends before it starts, is refused.
    let outside = OplogCursor {
        next_oplog_index: indices[9].as_u64(),
        current_component_revision: updated_revision.into(),
        query_hash: OplogCursor::query_hash(None),
    };
    assert!(
        read(Some(outside), from, to, 10, allowance(100))
            .await
            .is_err()
    );
    assert!(read(None, to, from, 10, allowance(100)).await.is_err());
}

#[test]
fn a_range_read_never_gets_more_than_the_configured_allowance() {
    let configured = ScanBudget {
        entries: std::num::NonZeroU64::new(50).unwrap(),
        time: std::time::Duration::from_secs(5),
    };
    assert_eq!(
        oplog_range_allowance(None, configured),
        OplogScanAllowance {
            entries: 50,
            millis: 5000
        }
    );
    assert_eq!(
        oplog_range_allowance(
            Some(OplogScanAllowance {
                entries: 7,
                millis: 9000
            }),
            configured
        ),
        OplogScanAllowance {
            entries: 7,
            millis: 5000
        }
    );
    assert_eq!(
        oplog_range_allowance(
            Some(OplogScanAllowance {
                entries: 0,
                millis: 0
            }),
            configured
        ),
        OplogScanAllowance {
            entries: 0,
            millis: 0
        }
    );
}
