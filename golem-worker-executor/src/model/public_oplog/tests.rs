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
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::payload::host_functions::HostFunctionName;
use golem_common::model::oplog::payload::types::{
    SerializableHttpErrorCode, SerializableHttpMethod, SerializableP3HttpBodyChunk,
    SerializableP3HttpClientSend, SerializableP3HttpClientSendResult,
    SerializableP3HttpConsumeBodyResult, SerializableP3HttpRequestOptions,
    SerializableP3HttpScheme, SerializableP3IpAddress, SerializableP3IpSocketAddress,
    SerializableP3SocketErrorCode, SerializableP3TcpChunk, SerializableP3UdpDatagram,
    SerializableResponseHeaders,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestP3HttpClientSend,
    HostRequestP3SocketsUdpSend, HostResponseP3BlobstoreIncomingValueStream,
    HostResponseP3HttpClientConsumeBodyChunk, HostResponseP3HttpClientConsumeBodyResult,
    HostResponseP3HttpClientSendResult, HostResponseP3KeyvalueIncomingValueStream,
    HostResponseP3SocketsTcpAcquire, HostResponseP3SocketsTcpReceiveChunk,
    HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend,
};
use golem_common::model::{
    AgentFingerprint, AgentMetadata, AgentStatusRecord, RetryConfig, Timestamp,
};
use golem_common::read_only_lock;
use golem_service_base::model::component::Component;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
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
                    address: SerializableP3IpAddress::IPv4 {
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
                        address: SerializableP3IpAddress::IPv6 {
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
        .read(
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
        let round_tripped: PublicOplogEntry = proto_entry
            .try_into()
            .unwrap_or_else(|err| panic!("protobuf decoding of entry {index} failed: {err}"));
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
