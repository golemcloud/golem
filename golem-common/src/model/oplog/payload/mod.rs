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

use crate::model::agent::RegisteredAgentType;
use crate::model::oplog::payload::types::{
    FileSystemError, ObjectMetadata, SerializableDateTime, SerializableFileTimes,
    SerializableSocketError,
};
use crate::model::oplog::public_oplog_entry::BinaryCodec;
use crate::model::oplog::types::{AgentMetadataForGuests, SerializableHttpErrorCode, SerializableHttpRequest, SerializableHttpResponse, SerializableInvokeRequest, SerializableInvokeResult, SerializableIpAddresses, SerializableRpcError, SerializableScheduledInvocation, SerializableStreamError};
use crate::model::oplog::PayloadId;
use crate::model::{
    ComponentId, ComponentVersion, ForkResult, OplogIndex, PromiseId, RevertWorkerTarget, WorkerId,
};
use crate::oplog_payload;
use crate::serialization::serialize;
use desert_rust::{
    BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_wasm::{IntoValueAndType, ValueAndType};
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::HashMap;
use std::fmt::Debug;
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
        GolemApiFork {
            name: String,
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
            target_version: ComponentVersion,
            mode: UpdateMode
        },
        GolemAgentGetAgentType {
            agent_type_name: String
        },
        GolemRpcInvoke {
            request: SerializableInvokeRequest
        },
        GolemRpcScheduledInvocation {
            invocation: SerializableScheduledInvocation
        },
        HttpRequest {
            request: SerializableHttpRequest
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
        GolemRpcInvokeAndAwait {
            result: Result<Option<ValueAndType>, SerializableRpcError>
        },
        GolemRpcInvokeGet {
            result: SerializableInvokeResult
        },
        GolemRpcScheduledInvocation {
            invocation: SerializableScheduledInvocation
        },
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OplogPayload<T: BinaryCodec + Debug + Clone + PartialEq> {
    Inline(T),
    SerializedInline(Vec<u8>),
    External {
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    },
}

impl<T: BinaryCodec + Debug + Clone + PartialEq> OplogPayload<T> {
    pub fn try_into_raw(self) -> Result<RawOplogPayload, String> {
        match self {
            OplogPayload::Inline(data) => {
                let bytes = serialize(&data)?;
                Ok(RawOplogPayload::SerializedInline(bytes))
            }
            OplogPayload::SerializedInline(bytes) => Ok(RawOplogPayload::SerializedInline(bytes)),
            OplogPayload::External {
                payload_id,
                md5_hash,
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
                let bytes = desert_rust::serialize_to_byte_vec(value)?;
                bytes.serialize(context)
            }
            OplogPayload::SerializedInline(bytes) => {
                context.write_u8(0);
                bytes.serialize(context)
            }
            OplogPayload::External {
                payload_id,
                md5_hash,
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
                Ok(Self::SerializedInline(bytes))
            }
            1 => {
                let payload_id = PayloadId::deserialize(context)?;
                let md5_hash = Vec::<u8>::deserialize(context)?;
                Ok(Self::External {
                    payload_id,
                    md5_hash,
                })
            }
            other => Err(desert_rust::Error::DeserializationFailure(format!(
                "Invalid tag for OplogPayload: {other}"
            ))),
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
            RawOplogPayload::SerializedInline(bytes) => Ok(OplogPayload::SerializedInline(bytes)),
            RawOplogPayload::External {
                payload_id,
                md5_hash,
            } => Ok(OplogPayload::External {
                payload_id,
                md5_hash,
            }),
        }
    }
}
