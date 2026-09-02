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
    EntityActivation, EntityActivationPolicy, ExecutableTarget, FilesystemCapability,
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
use golem_common::schema::{IntoTypedSchemaValue, SecretValuePayload};
use golem_service_base::model::component::Component;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use prost::Message;
use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;
use test_r::test;
use uuid::Uuid;

/// Component service stub for entries whose rendering must not need component
/// metadata (`Start`/`End`/`Cancelled` host call entries).
struct PanicComponentService;

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
            binding: Box::new(CompiledToolBinding {
                deployment_revision,
                agent_type_name: AgentTypeName("Agent".to_string()),
                tool_name: tool_name.clone(),
                version: "1".to_string(),
                metadata_version: "1".to_string(),
                account_id: AccountId::new(),
                account_email: AccountEmail::new("owner@example.com"),
                parameters: NormalizedJsonValue::new(serde_json::json!({})),
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
    let metadata = EntityInvocationRequest {
        activation: test_entity_activation(&entity),
        entity,
        calling_principal: Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        }),
        call_mode,
        operation,
        principal: None,
    };
    HostRequestEntityInvocation {
        metadata: desert_rust::serialize_to_byte_vec(&metadata).unwrap(),
        input,
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
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;
    let timestamp = Timestamp::now_utc();
    assert_eq!(
        oplog
            .add(OplogEntry::NoOp {
                timestamp,
                entity_parent_start_index: None,
            })
            .await,
        OplogIndex::INITIAL
    );
    oplog.commit(CommitLevel::Always).await;

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
    assert!(matches!(chunk.entries[0].entry, PublicOplogEntry::NoOp(_)));
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
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let agent_entry = oplog.add(OplogEntry::no_op(None)).await;
    let observational_owner = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("agent-custom-owner".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteLocal,
        })
        .await;
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
        })
        .await;

    let interleaved_agent_entry = oplog.add(OplogEntry::no_op(None)).await;
    let child_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(middleware_start),
            function_name: HostFunctionName::Custom("entity-child".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequestNoInput {}.into()))),
            durable_function_type: DurableFunctionType::ReadLocal,
        })
        .await;

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
            command_path: vec!["files".to_string(), "lookup".to_string()],
            args: vec!["configured-secret-rendering".to_string()],
            has_stdin: true,
            has_stdout: true,
            declares_stdout: true,
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
        })
        .await;
    let entity_retry_error = oplog
        .add(OplogEntry::error(
            Some(tool_start),
            golem_common::model::oplog::AgentError::TransientError("entity retry".to_string()),
            agent_entry,
            false,
            None,
        ))
        .await;
    let entity_marker = oplog.add(OplogEntry::no_op(Some(tool_start))).await;
    let log_index = oplog
        .add(OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            level: LogLevel::Info,
            context: "tool".to_string(),
            message: "entity-attribution-needle".to_string(),
        })
        .await;
    let span_id = SpanId::generate();
    let span_index = oplog
        .add(OplogEntry::StartSpan {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            span_id,
            parent: None,
            linked_context_id: None,
            attributes: AttributeMap(HashMap::new()),
        })
        .await;
    let stream_frame_index = oplog
        .add(OplogEntry::HostStreamFrame {
            timestamp: Timestamp::now_utc(),
            parent_start_index: tool_start,
            kind: HostStreamKind::P3HttpRequestBody,
            payload: OplogPayload::Inline(Box::new(HostRequestNoInput {}.into())),
        })
        .await;

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
        })
        .await;
    let observational_log = oplog
        .add(OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(observational_start),
            level: LogLevel::Info,
            context: "custom".to_string(),
            message: "agent-owned observation".to_string(),
        })
        .await;
    let observational_end = oplog
        .add(OplogEntry::end(observational_start, None, false))
        .await;

    let transaction_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            function_name: HostFunctionName::Custom("transaction".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteRemoteTransaction(None),
        })
        .await;
    let transaction_begin = oplog
        .add(OplogEntry::BeginRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            transaction_id: TransactionId::new("entity-transaction".to_string()),
            original_begin_index: None,
        })
        .await;
    let transaction_commit = oplog
        .add(OplogEntry::CommittedRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index: transaction_start,
        })
        .await;
    let transaction_end = oplog
        .add(OplogEntry::end(transaction_start, None, false))
        .await;
    let child_end = oplog.add(OplogEntry::end(child_start, None, false)).await;
    let tool_terminal = SerializableToolOperationTerminal {
        body_execution: SerializableEntityBodyExecution::Executed,
        result: Ok(SerializableToolStructuredResult { result: None }),
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
        ))
        .await;
    let completion = oplog
        .add(OplogEntry::completion_delivered(tool_start))
        .await;

    let rejected_request: HostRequest = HostRequestGolemToolInvocationRejected {
        tool_name: "rejected".to_string(),
        command_path: vec!["reject".to_string()],
        input: None,
        input_decode_failure: None,
        has_stdin: false,
        has_stdout: false,
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
        })
        .await;
    let rejected_end = oplog
        .add(OplogEntry::end(rejected_start, None, false))
        .await;
    let middleware_end = oplog
        .add(OplogEntry::end(middleware_start, None, false))
        .await;
    let final_log = oplog
        .add(OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(tool_start),
            level: LogLevel::Info,
            context: "tool".to_string(),
            message: "last-entity-attribution-needle".to_string(),
        })
        .await;
    oplog.commit(CommitLevel::Always).await;

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
        span_index,
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
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id, account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let non_start = oplog.add(OplogEntry::no_op(None)).await;
    let non_entity_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("not-an-entity".to_string()),
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteLocal,
        })
        .await;
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
        })
        .await;
    let valid = oplog.add(OplogEntry::no_op(Some(entity_start))).await;
    let invalid_non_start = oplog.add(OplogEntry::no_op(Some(non_start))).await;
    let invalid_non_entity = oplog.add(OplogEntry::no_op(Some(non_entity_start))).await;
    let invalid_forward_index = invalid_non_entity.next();
    let future_entity_start = invalid_forward_index.next();
    assert_eq!(
        oplog
            .add(OplogEntry::no_op(Some(future_entity_start)))
            .await,
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
            })
            .await,
        future_entity_start
    );
    oplog.commit(CommitLevel::Always).await;

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
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

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
    }
    .into_typed_schema_value()
    .unwrap();
    let entity_request: HostRequest = HostRequestEntityInvocation {
        metadata: vec![1, 2, 3],
        input: entity_input.clone(),
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
        })
        .await;
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
        ))
        .await;
    oplog.commit(CommitLevel::Always).await;

    let last_index = oplog_service
        .get_last_index(&owned_agent_id, AgentMode::Durable)
        .await;
    let raw_entries = oplog_service
        .read_exact(
            &owned_agent_id,
            AgentMode::Durable,
            OplogIndex::INITIAL,
            Into::<u64>::into(last_index),
        )
        .await;

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
