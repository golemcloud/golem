// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

pub mod types;

#[cfg(test)]
mod tests;

use crate::model::agent::{AgentTypeName, DataValue, RegisteredAgentType, UntypedDataValue};
use crate::model::component::ComponentRevision;
use crate::model::oplog::PayloadId;
use crate::model::oplog::payload::types::{
    FileSystemError, ObjectMetadata, SerializableDateTime, SerializableFileTimes,
    SerializableSocketError,
};
use crate::model::oplog::types::{
    AgentMetadataForGuests, SerializableDbColumn, SerializableDbResult, SerializableDbValue,
    SerializableHttpErrorCode, SerializableHttpMethod, SerializableHttpResponse,
    SerializableInvokeResult, SerializableIpAddresses, SerializableRdbmsError,
    SerializableRdbmsRequest, SerializableRpcError, SerializableScheduledInvocation,
    SerializableStreamError,
};
use crate::model::worker::RevertWorkerTarget;
use crate::model::{ComponentId, ForkResult, IdempotencyKey, OplogIndex, PromiseId, WorkerId};
use crate::oplog_payload;
use crate::serialization::serialize;
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer,
    DeserializationContext, SerializationContext,
};
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_wasm::{IntoValueAndType, ValueAndType};
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use uuid::Uuid;

oplog_payload! {
    HostRequest => {
        NoInput {},
        BlobStoreContainer {
            container: String
        },
        BlobStoreContainerAndObject {
            container: String,
            object: String
        },
        BlobStoreContainerAndObjects {
            container: String,
            objects: Vec<String>
        },
        BlobStoreGetData {
            container: String,
            object: String,
            begin: u64,
            end: u64,
        },
        BlobStoreWriteData {
            container: String,
            object: String,
            length: u64
        },
        BlobStoreCopyOrMove {
            source_container: String,
            source_object: String,
            target_container: String,
            target_object: String
        },
        MonotonicClockDuration {
            duration_in_nanos: u64
        },
        FileSystemPath {
            path: String
        },
        GolemApiAgentId {
            agent_id: WorkerId
        },
        GolemApiComponentSlug {
            component_slug: String
        },
        GolemApiComponentSlugAndAgentName {
            component_slug: String,
            agent_name: String
        },
        GolemApiForkAgent {
            source_agent_id: WorkerId,
            target_agent_id: WorkerId,
            oplog_index_cut_off: OplogIndex
        },
        GolemApiPromiseId {
            promise_id: PromiseId
        },
        GolemApiRevertAgent {
            agent_id: WorkerId,
            target: RevertWorkerTarget
        },
        GolemApiUpdateAgent {
            agent_id: WorkerId,
            target_revision: ComponentRevision,
            mode: UpdateMode
        },
        GolemAgentGetAgentType {
            agent_type_name: AgentTypeName
        },
        GolemRdbmsRequest {
            request: Option<SerializableRdbmsRequest>
        },
        GolemRpcInvoke {
            remote_worker_id: WorkerId,
            idempotency_key: IdempotencyKey,
            method_name: String,
            input: UntypedDataValue,
            #[from_value(skip)]
            #[transient(None::<AgentTypeName>)]
            remote_agent_type: Option<AgentTypeName>, // enriched field, only filled when exposed as public oplog entry
            #[transient(None::<DataValue>)]
            #[from_value(skip)]
            remote_agent_parameters: Option<DataValue>, // enriched field, only filled when exposed as public oplog entry
        },
        GolemRpcScheduledInvocation {
            remote_worker_id: WorkerId,
            idempotency_key: IdempotencyKey,
            method_name: String,
            input: UntypedDataValue,
            datetime: SerializableDateTime,
            #[from_value(skip)]
            #[transient(None::<AgentTypeName>)]
            remote_agent_type: Option<AgentTypeName>, // enriched field, only filled when exposed as public oplog entry
            #[from_value(skip)]
            #[transient(None::<DataValue>)]
            remote_agent_parameters: Option<DataValue>, // enriched field, only filled when exposed as public oplog entry
        },
        GolemRpcScheduledInvocationCancellation {
            invocation: SerializableScheduledInvocation
        },
        HttpRequest {
             uri: String,
             method: SerializableHttpMethod,
             headers: HashMap<String, String>,
        },
        KVBucket {
            bucket: String
        },
        KVBucketAndKey {
            bucket: String,
            key: String
        },
        KVBucketAndKeys {
            bucket: String,
            keys: Vec<String>
        },
        KVBucketKeyAndSize {
            bucket: String,
            key: String,
            length: usize
        },
        KVBucketAndKeySizePairs {
            bucket: String,
            keys: Vec<(String, usize)>,
        },
        PollCount {
            count: usize
        },
        RandomBytes {
            length: u64
        },
        SocketsResolveName {
            name: String
        },
    }
}

oplog_payload! {
    HostResponse => {
        BlobStoreUnit {
            result: Result<(), String>,
        },
        BlobStoreGetData {
            result: Result<Vec<u8>, String>,
        },
        BlobStoreListObjects {
            result: Result<Vec<String>, String>
        },
        BlobStoreContains {
            result: Result<bool, String>
        },
        BlobStoreObjectMetadata {
            result: Result<ObjectMetadata, String>
        },
        BlobStoreTimestamp {
            result: Result<u64, String>
        },
        BlobStoreOptionalTimestamp {
            result: Result<Option<u64>, String>
        },
        MonotonicClockTimestamp {
            nanos: u64
        },
        WallClock {
            time: SerializableDateTime,
        },
        FileSystemStat {
            result: Result<SerializableFileTimes, FileSystemError>,
        },
        GolemAgentWebhookUrl {
            result: Result<String, String>
        },
        GolemApiAgentId {
            result: Result<Option<WorkerId>, String>
        },
        GolemApiAgentMetadata {
            metadata: Option<AgentMetadataForGuests>
        },
        GolemApiSelfAgentMetadata {
            metadata: AgentMetadataForGuests
        },
        GolemApiComponentId {
            result: Result<Option<ComponentId>, String>
        },
        GolemApiFork {
            forked_phantom_id: Uuid,
            result: Result<ForkResult, String>,
        },
        GolemApiIdempotencyKey {
            uuid: Uuid
        },
        GolemApiPromiseId {
            promise_id: PromiseId
        },
        GolemApiPromiseCompletion {
            completed: bool
        },
        GolemApiPromiseResult {
            result: Option<Vec<u8>>
        },
        GolemApiUnit {
            result: Result<(), String>,
        },
        GolemAgentAgentTypes {
            result: Result<Vec<RegisteredAgentType>, String>
        },
        GolemAgentAgentType {
            result: Result<Option<RegisteredAgentType>, String>
        },
        GolemRdbmsColumns {
            result: Result<Vec<SerializableDbColumn>, SerializableRdbmsError>
        },
        GolemRdbmsRowCount {
            result: Result<u64, SerializableRdbmsError>
        },
        GolemRdbmsResult {
            result: Result<SerializableDbResult, SerializableRdbmsError>
        },
        GolemRdbmsResultChunk {
            result: Result<Option<Vec<Vec<SerializableDbValue>>>, SerializableRdbmsError>
        },
        GolemRdbmsRequest {
            request: Result<SerializableRdbmsRequest, SerializableRdbmsError>
        },
        GolemRpcInvokeAndAwait {
            result: Result<UntypedDataValue, SerializableRpcError>
        },
        GolemRpcInvokeGet {
            result: SerializableInvokeResult
        },
        GolemRpcScheduledInvocation {
            invocation: SerializableScheduledInvocation
        },
        GolemRpcUnitOrFailure { result: Result<(), SerializableRpcError> },
        GolemRpcUnit {},
        HttpFutureTrailersGet {
            result:  Result<Option<Result<Result<Option<HashMap<String, Vec<u8>>>, SerializableHttpErrorCode>, ()>>, String>
        },
        HttpResponse {
            response: SerializableHttpResponse
        },
        KVGet {
            result: Result<Option<Vec<u8>>, String>
        },
        KVGetMany {
            result: Result<Vec<Option<Vec<u8>>>, String>
        },
        KVDelete {
            result: Result<bool, String>
        },
        KVKeys {
            result: Result<Vec<String>, String>
        },
        KVUnit {
            result: Result<(), String>
        },
        PollReady {
            result: Result<bool, String>
        },
        PollResult {
            result: Result<Vec<u32>, String>
        },
        RandomBytes {
            bytes: Vec<u8>
        },
        RandomU64 {
            value: u64
        },
        RandomSeed {
            lo: u64,
            hi: u64
        },
        SocketsResolveName {
            result: Result<SerializableIpAddresses, SerializableSocketError>
        },
        StreamChunk {
            result: Result<Vec<u8>, SerializableStreamError>
        },
        StreamSkip {
            result: Result<u64, SerializableStreamError>
        }
    }
}

pub trait HostPayloadPair {
    type Req: Into<HostRequest>;
    type Resp: Into<HostResponse> + TryFrom<HostResponse, Error = String>;

    const INTERFACE: &'static str;
    const FUNCTION: &'static str;
    const FQFN: &'static str;

    const HOST_FUNCTION_NAME: host_functions::HostFunctionName;
}

pub mod host_functions {
    use crate::host_payload_pairs;

    host_payload_pairs! {
        (RdbmsMysqlDbConnectionExecute => "rdbms::mysql::db-connection", "execute", GolemRdbmsRequest, GolemRdbmsRowCount),
        (RdbmsMysqlDbConnectionQuery => "rdbms::mysql::db-connection", "query", GolemRdbmsRequest, GolemRdbmsResult),
        (RdbmsMysqlDbConnectionQueryStream => "rdbms::mysql::db-connection", "query-stream", NoInput, GolemRdbmsRequest),
        (RdbmsMysqlDbResultStreamGetColumns => "rdbms::mysql::db-result-stream", "get-columns", NoInput, GolemRdbmsColumns),
        (RdbmsMysqlDbResultStreamGetNext => "rdbms::mysql::db-result-stream", "get-next", NoInput, GolemRdbmsResultChunk),
        (RdbmsMysqlDbTransactionQuery => "rdbms::mysql::db-transaction", "query", GolemRdbmsRequest, GolemRdbmsResult),
        (RdbmsMysqlDbTransactionExecute => "rdbms::mysql::db-transaction", "execute", GolemRdbmsRequest, GolemRdbmsRowCount),
        (RdbmsMysqlDbTransactionQueryStream => "rdbms::mysql::db-transaction", "query-stream", NoInput, GolemRdbmsRequest),
        (RdbmsPostgresDbConnectionExecute => "rdbms::postgres::db-connection", "execute", GolemRdbmsRequest, GolemRdbmsRowCount),
        (RdbmsPostgresDbConnectionQuery => "rdbms::postgres::db-connection", "query", GolemRdbmsRequest, GolemRdbmsResult),
        (RdbmsPostgresDbConnectionQueryStream => "rdbms::postgres::db-connection", "query-stream", NoInput, GolemRdbmsRequest),
        (RdbmsPostgresDbResultStreamGetColumns => "rdbms::postgres::db-result-stream", "get-columns", NoInput, GolemRdbmsColumns),
        (RdbmsPostgresDbResultStreamGetNext => "rdbms::postgres::db-result-stream", "get-next", NoInput, GolemRdbmsResultChunk),
        (RdbmsPostgresDbTransactionQuery => "rdbms::postgres::db-transaction", "query", GolemRdbmsRequest, GolemRdbmsResult),
        (RdbmsPostgresDbTransactionExecute => "rdbms::postgres::db-transaction", "execute", GolemRdbmsRequest, GolemRdbmsRowCount),
        (RdbmsPostgresDbTransactionQueryStream => "rdbms::postgres::db-transaction", "query-stream", NoInput, GolemRdbmsRequest),
        (KeyvalueEventualGet => "keyvalue::eventual", "get", KVBucketAndKey, KVGet),
        (KeyvalueEventualSet => "keyvalue::eventual", "set", KVBucketKeyAndSize, KVUnit),
        (KeyvalueEventualDelete => "keyvalue::eventual", "delete", KVBucketAndKey, KVUnit),
        (KeyvalueEventualExists => "keyvalue::eventual", "exists", KVBucketAndKey, KVDelete),
        (KeyvalueEventualBatchGetMany => "keyvalue::eventual_batch", "get_many", KVBucketAndKeys, KVGetMany),
        (KeyvalueEventualBatchGetKeys => "keyvalue::eventual_batch", "get_keys", KVBucket, KVKeys),
        (KeyvalueEventualBatchSetMany => "keyvalue::eventual_batch", "set_many", KVBucketAndKeySizePairs, KVUnit),
        (KeyvalueEventualBatchDeleteMany => "keyvalue::eventual_batch", "delete_many", KVBucketAndKeys, KVUnit),
        (RandomInsecureSeedInsecureSeed => "random::insecure_seed", "insecure_seed", NoInput, RandomSeed),
        (RandomInsecureGetInsecureRandomBytes => "random::insecure", "get_insecure_random_bytes", RandomBytes, RandomBytes),
        (RandomInsecureGetInsecureRandomU64 => "random::insecure", "get_insecure_random_u64", NoInput, RandomU64),
        (RandomGetRandomBytes => "random", "get_random_bytes", RandomBytes, RandomBytes),
        (RandomGetRandomU64 => "random", "get_random_u64", NoInput, RandomU64),
        (GolemRpcFutureInvokeResultGet => "golem::rpc::future-invoke-result", "get", GolemRpcInvoke, GolemRpcInvokeGet),
        (GolemRpcWasmRpcInvokeAndAwaitResult => "golem::rpc::wasm-rpc", "invoke_and_await", GolemRpcInvoke, GolemRpcInvokeAndAwait),
        (GolemRpcWasmRpcInvoke => "golem::rpc::wasm-rpc", "invoke", GolemRpcInvoke, GolemRpcUnitOrFailure),
        (GolemRpcWasmRpcScheduleInvocation => "golem::rpc::wasm-rpc", "schedule_invocation", GolemRpcScheduledInvocation, GolemRpcScheduledInvocation),
        (GolemRpcCancellationTokenCancel => "golem::rpc::cancellation-token", "cancel", GolemRpcScheduledInvocationCancellation, GolemRpcUnit),
        (IoPollReady => "io::poll", "ready", NoInput, PollReady),
        (IoPollPoll => "io::poll", "poll", PollCount, PollResult),
        (HttpTypesFutureTrailersGet => "http::types::future_trailers", "get", HttpRequest, HttpFutureTrailersGet),
        (HttpTypesFutureIncomingResponseGet => "http::types::future_incoming_response", "get", HttpRequest, HttpResponse),
        (HttpTypesIncomingBodyStreamRead => "http::types::incoming_body_stream", "read", HttpRequest, StreamChunk),
        (HttpTypesIncomingBodyStreamBlockingRead => "http::types::incoming_body_stream", "blocking_read", HttpRequest, StreamChunk),
        (HttpTypesIncomingBodyStreamSkip => "http::types::incoming_body_stream", "skip", HttpRequest, StreamSkip),
        (HttpTypesIncomingBodyStreamBlockingSkip => "http::types::incoming_body_stream", "blocking_skip", HttpRequest, StreamSkip),
        (WallClockNow => "wall_clock", "now", NoInput, WallClock),
        (WallClockResolution => "wall_clock", "resolution", NoInput, WallClock),
        (MonotonicClockNow => "monotonic_clock", "now", NoInput, MonotonicClockTimestamp),
        (MonotonicClockResolution => "monotonic_clock", "resolution", NoInput, MonotonicClockTimestamp),
        (MonotonicClockSubscribeDuration => "monotonic_clock", "subscribe_duration", MonotonicClockDuration, MonotonicClockTimestamp),
        (BlobstoreBlobstoreCreateContainer => "blobstore::blobstore", "create_container", BlobStoreContainer, BlobStoreTimestamp),
        (BlobstoreBlobstoreGetContainer => "blobstore::blobstore", "get_container", BlobStoreContainer, BlobStoreOptionalTimestamp),
        (BlobstoreBlobstoreDeleteContainer => "blobstore::blobstore", "delete_container", BlobStoreContainer, BlobStoreUnit),
        (BlobstoreBlobstoreContainerExists => "blobstore::blobstore", "container_exists", BlobStoreContainer, BlobStoreContains),
        (BlobstoreBlobstoreCopyObject => "blobstore::blobstore", "copy_object", BlobStoreCopyOrMove, BlobStoreUnit),
        (BlobstoreBlobstoreMoveObject => "blobstore::blobstore", "move_object", BlobStoreCopyOrMove, BlobStoreUnit),
        (BlobstoreContainerGetData => "blobstore::container", "get_data", BlobStoreGetData, BlobStoreGetData),
        (BlobstoreContainerWriteData => "blobstore::container", "write_data", BlobStoreWriteData, BlobStoreUnit),
        (BlobstoreContainerListObject => "blobstore::container", "list_object", BlobStoreContainer, BlobStoreListObjects),
        (BlobstoreContainerDeleteObject => "blobstore::container", "delete_object", BlobStoreContainerAndObject, BlobStoreUnit),
        (BlobstoreContainerDeleteObjects => "blobstore::container", "delete_objects", BlobStoreContainerAndObjects, BlobStoreUnit),
        (BlobstoreContainerHasObject => "blobstore::container", "has_object", BlobStoreContainerAndObject, BlobStoreContains),
        (BlobstoreContainerObjectInfo => "blobstore::container", "object_info", BlobStoreContainerAndObject, BlobStoreObjectMetadata),
        (BlobstoreContainerClear => "blobstore::container", "clear", BlobStoreContainer, BlobStoreUnit),
        (FilesystemTypesDescriptorStat => "filesystem::types::descriptor", "stat", FileSystemPath, FileSystemStat),
        (FilesystemTypesDescriptorStatAt => "filesystem::types::descriptor", "stat_at", FileSystemPath, FileSystemStat),
        (SocketsIpNameLookupResolveAddresses => "sockets::ip_name_lookup", "resolve_addresses", SocketsResolveName, SocketsResolveName),
        (GolemAgentGetAllAgentTypes => "golem::agent", "get_all_agent_types", NoInput, GolemAgentAgentTypes),
        (GolemAgentGetAgentType => "golem::agent", "get_agent_type", GolemAgentGetAgentType, GolemAgentAgentType),
        (GolemAgentCreateWebhook => "golem::agent", "create_webhook", GolemApiPromiseId, GolemAgentWebhookUrl),
        (GolemApiCreatePromise => "golem::api", "create_promise", NoInput, GolemApiPromiseId),
        (GolemApiCompletePromise => "golem::api", "complete_promise", GolemApiPromiseId, GolemApiPromiseCompletion),
        (GolemApiGenerateIdempotencyKey => "golem::api", "generate_idempotency-key", NoInput, GolemApiIdempotencyKey),
        (GolemApiUpdateWorker => "golem::api", "update_worker", GolemApiUpdateAgent, GolemApiUnit),
        (GolemApiGetSelfMetadata => "golem::api", "get_self_metadata", NoInput, GolemApiSelfAgentMetadata),
        (GolemApiGetAgentMetadata => "golem::api", "get_agent_metadata", GolemApiAgentId, GolemApiAgentMetadata),
        (GolemApiGetPromiseResult => "golem::api", "get_promise_result", NoInput, GolemApiPromiseResult),
        (GolemApiForkWorker => "golem::api", "fork_worker", GolemApiForkAgent, GolemApiUnit),
        (GolemApiRevertWorker => "golem::api", "revert_worker", GolemApiRevertAgent, GolemApiUnit),
        (GolemApiResolveComponentId => "golem::api", "resolve_component_id", GolemApiComponentSlug, GolemApiComponentId),
        (GolemApiResolveWorkerIdStrict => "golem::api", "resolve_worker_id_strict", GolemApiComponentSlugAndAgentName, GolemApiAgentId),
        (GolemApiFork => "golem::api", "fork", NoInput, GolemApiFork)
    }
}

impl golem_wasm::IntoValue for host_functions::HostFunctionName {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::String(self.to_string())
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        golem_wasm::analysis::analysed_type::str()
    }
}

impl golem_wasm::FromValue for host_functions::HostFunctionName {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::String(s) => Ok(Self::from(s.as_str())),
            other => Err(format!(
                "Expected String for HostFunctionName, got {other:?}"
            )),
        }
    }
}

pub enum OplogPayload<T: BinaryCodec + Debug + Clone + PartialEq> {
    Inline(Box<T>),
    SerializedInline {
        bytes: Vec<u8>,
        /// In-memory cache of the deserialized value. Not serialized to disk/network.
        cached: Option<Arc<T>>,
    },
    External {
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
        /// In-memory cache of the deserialized value. Not serialized to disk/network.
        cached: Option<Arc<T>>,
    },
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> Clone for OplogPayload<T> {
    fn clone(&self) -> Self {
        match self {
            OplogPayload::Inline(v) => OplogPayload::Inline(v.clone()),
            OplogPayload::SerializedInline { bytes, cached } => OplogPayload::SerializedInline {
                bytes: bytes.clone(),
                cached: cached.clone(),
            },
            OplogPayload::External {
                payload_id,
                md5_hash,
                cached,
            } => OplogPayload::External {
                payload_id: payload_id.clone(),
                md5_hash: md5_hash.clone(),
                cached: cached.clone(),
            },
        }
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> PartialEq for OplogPayload<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (OplogPayload::Inline(a), OplogPayload::Inline(b)) => a == b,
            (
                OplogPayload::SerializedInline { bytes: a, .. },
                OplogPayload::SerializedInline { bytes: b, .. },
            ) => a == b,
            (
                OplogPayload::External {
                    payload_id: a_id,
                    md5_hash: a_md5,
                    ..
                },
                OplogPayload::External {
                    payload_id: b_id,
                    md5_hash: b_md5,
                    ..
                },
            ) => a_id == b_id && a_md5 == b_md5,
            _ => false,
        }
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> Eq for OplogPayload<T> {}

impl<T: BinaryCodec + Debug + Clone + PartialEq> Debug for OplogPayload<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OplogPayload::Inline(v) => f.debug_tuple("Inline").field(v).finish(),
            OplogPayload::SerializedInline { bytes, cached } => f
                .debug_struct("SerializedInline")
                .field("bytes_len", &bytes.len())
                .field("cached", &cached.is_some())
                .finish(),
            OplogPayload::External {
                payload_id,
                md5_hash,
                cached,
            } => f
                .debug_struct("External")
                .field("payload_id", payload_id)
                .field("md5_hash", md5_hash)
                .field("cached", &cached.is_some())
                .finish(),
        }
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> OplogPayload<T> {
    pub fn try_into_raw(self) -> Result<RawOplogPayload, String> {
        match self {
            OplogPayload::Inline(data) => {
                let bytes = serialize(&data)?;
                Ok(RawOplogPayload::SerializedInline(bytes))
            }
            OplogPayload::SerializedInline { bytes, .. } => {
                Ok(RawOplogPayload::SerializedInline(bytes))
            }
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => Ok(RawOplogPayload::External {
                payload_id,
                md5_hash,
            }),
        }
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> BinarySerializer for OplogPayload<T> {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match self {
            OplogPayload::Inline(value) => {
                context.write_u8(0);
                let bytes = serialize(value).map_err(desert_rust::Error::SerializationFailure)?;
                bytes.serialize(context)
            }
            OplogPayload::SerializedInline { bytes, .. } => {
                context.write_u8(0);
                bytes.serialize(context)
            }
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => {
                context.write_u8(1);
                payload_id.serialize(context)?;
                md5_hash.serialize(context)
            }
        }
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> BinaryDeserializer for OplogPayload<T> {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag = context.read_u8()?;
        match tag {
            0 => {
                let bytes = Vec::<u8>::deserialize(context)?;
                Ok(Self::SerializedInline {
                    bytes,
                    cached: None,
                })
            }
            1 => {
                let payload_id = PayloadId::deserialize(context)?;
                let md5_hash = Vec::<u8>::deserialize(context)?;
                Ok(Self::External {
                    payload_id,
                    md5_hash,
                    cached: None,
                })
            }
            other => Err(desert_rust::Error::DeserializationFailure(format!(
                "Invalid tag for OplogPayload: {other}"
            ))),
        }
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> golem_wasm::IntoValue for OplogPayload<T> {
    fn into_value(self) -> golem_wasm::Value {
        match self {
            OplogPayload::Inline(value) => {
                let bytes = serialize(&value).expect("Failed to serialize OplogPayload::Inline");
                golem_wasm::Value::Variant {
                    case_idx: 0,
                    case_value: Some(Box::new(bytes.into_value())),
                }
            }
            OplogPayload::SerializedInline { bytes, .. } => golem_wasm::Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(bytes.into_value())),
            },
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => golem_wasm::Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(golem_wasm::Value::Record(vec![
                    payload_id.0.into_value(),
                    md5_hash.into_value(),
                ]))),
            },
        }
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        use golem_wasm::analysis::analysed_type::*;
        let uuid_type = record(vec![field("high-bits", u64()), field("low-bits", u64())])
            .named("uuid")
            .owned("golem:core@1.5.0/types");
        variant(vec![
            case("inline", list(u8())),
            case(
                "external",
                record(vec![
                    field("payload-id", uuid_type),
                    field("md5-hash", list(u8())),
                ])
                .named("oplog-external-payload")
                .owned("golem:api@1.5.0/oplog"),
            ),
        ])
        .named("oplog-payload")
        .owned("golem:api@1.5.0/oplog")
    }
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> golem_wasm::FromValue for OplogPayload<T> {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Variant {
                case_idx,
                case_value,
            } => match case_idx {
                0 => {
                    let bytes = Vec::<u8>::from_value(
                        *case_value.ok_or("Expected case_value for inline")?,
                    )?;
                    Ok(OplogPayload::SerializedInline {
                        bytes,
                        cached: None,
                    })
                }
                1 => {
                    let record_value = *case_value.ok_or("Expected case_value for external")?;
                    match record_value {
                        golem_wasm::Value::Record(fields) if fields.len() == 2 => {
                            let mut iter = fields.into_iter();
                            let payload_id = PayloadId(Uuid::from_value(iter.next().unwrap())?);
                            let md5_hash = Vec::<u8>::from_value(iter.next().unwrap())?;
                            Ok(OplogPayload::External {
                                payload_id,
                                md5_hash,
                                cached: None,
                            })
                        }
                        other => Err(format!(
                            "Expected Record with 2 fields for oplog-external-payload, got {other:?}"
                        )),
                    }
                }
                _ => Err(format!("Invalid case_idx for OplogPayload: {case_idx}")),
            },
            other => Err(format!("Expected Variant for OplogPayload, got {other:?}")),
        }
    }
}

/// Untyped version of OplogPayload
pub enum RawOplogPayload {
    SerializedInline(Vec<u8>),
    External {
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    },
}

impl RawOplogPayload {
    pub fn into_payload<T: BinaryCodec + Debug + Clone + PartialEq>(
        self,
    ) -> Result<OplogPayload<T>, String> {
        match self {
            RawOplogPayload::SerializedInline(bytes) => Ok(OplogPayload::SerializedInline {
                bytes,
                cached: None,
            }),
            RawOplogPayload::External {
                payload_id,
                md5_hash,
            } => Ok(OplogPayload::External {
                payload_id,
                md5_hash,
                cached: None,
            }),
        }
    }

    pub fn into_payload_with_cache<T: BinaryCodec + Debug + Clone + PartialEq>(
        self,
        cached: Arc<T>,
    ) -> Result<OplogPayload<T>, String> {
        match self {
            RawOplogPayload::SerializedInline(bytes) => Ok(OplogPayload::SerializedInline {
                bytes,
                cached: Some(cached),
            }),
            RawOplogPayload::External {
                payload_id,
                md5_hash,
            } => Ok(OplogPayload::External {
                payload_id,
                md5_hash,
                cached: Some(cached),
            }),
        }
    }
}
