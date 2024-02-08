// Copyright 2024 Golem Cloud
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

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::ops::Add;
use std::str::FromStr;
use std::time::Duration;

use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use bytes::Bytes;
use derive_more::FromStr;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ParseFromParameter, ParseResult, ToJSON};
use poem_openapi::{Enum, Object};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

use crate::newtype_uuid;
use crate::serialization::{
    deserialize_with_version, serialize, try_deserialize, SERIALIZATION_VERSION_V1,
};

newtype_uuid!(
    TemplateId,
    golem_api_grpc::proto::golem::template::TemplateId
);

newtype_uuid!(ProjectId, golem_api_grpc::proto::golem::common::ProjectId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Timestamp(iso8601_timestamp::Timestamp);

impl Timestamp {
    pub fn now_utc() -> Timestamp {
        Timestamp(iso8601_timestamp::Timestamp::now_utc())
    }
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            iso8601_timestamp::Timestamp::deserialize(deserializer).map(Self)
        } else {
            // For non-human-readable formats we assume it was an i64 representing milliseconds from epoch
            let timestamp = i64::deserialize(deserializer)?;
            Ok(Timestamp(
                iso8601_timestamp::Timestamp::UNIX_EPOCH
                    .add(Duration::from_millis(timestamp as u64)),
            ))
        }
    }
}

impl bincode::Encode for Timestamp {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        (self
            .0
            .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH)
            .whole_milliseconds() as i64)
            .encode(encoder)
    }
}

impl bincode::Decode for Timestamp {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let timestamp: i64 = bincode::Decode::decode(decoder)?;
        Ok(Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(timestamp as u64)),
        ))
    }
}

impl<'de> bincode::BorrowDecode<'de> for Timestamp {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let timestamp: i64 = bincode::BorrowDecode::borrow_decode(decoder)?;
        Ok(Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(timestamp as u64)),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct VersionedWorkerId {
    #[serde(rename = "instance_id")]
    pub worker_id: WorkerId,
    #[serde(rename = "component_version")]
    pub template_version: i32,
}

impl VersionedWorkerId {
    pub fn slug(&self) -> String {
        format!("{}#{}", self.worker_id.slug(), self.template_version)
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| panic!("failed to serialize versioned worker id: {self}"))
    }
}

impl Display for VersionedWorkerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.slug())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct WorkerId {
    #[serde(rename = "component_id")]
    pub template_id: TemplateId,
    #[serde(rename = "instance_name")]
    pub worker_name: String,
}

impl WorkerId {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.template_id, self.worker_name)
    }

    pub fn into_proto(self) -> golem_api_grpc::proto::golem::worker::WorkerId {
        golem_api_grpc::proto::golem::worker::WorkerId {
            template_id: Some(self.template_id.into()),
            name: self.worker_name,
        }
    }

    pub fn from_proto(proto: golem_api_grpc::proto::golem::worker::WorkerId) -> Self {
        Self {
            template_id: proto.template_id.unwrap().try_into().unwrap(),
            worker_name: proto.name,
        }
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| panic!("failed to serialize worker id {self}"))
    }

    pub fn to_redis_key(&self) -> String {
        format!("{}:{}", self.template_id.0, self.worker_name)
    }

    pub fn uri(&self) -> String {
        format!("worker://{}", self.slug())
    }
}

impl FromStr for WorkerId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let template_id_uuid = Uuid::from_str(parts[0])
                .map_err(|_| format!("invalid template id: {s} - expected uuid"))?;
            let template_id = TemplateId(template_id_uuid);
            let worker_name = parts[1].to_string();
            Ok(Self {
                template_id,
                worker_name,
            })
        } else {
            Err(format!(
                "invalid worker id: {s} - expected format: <template_id>:<worker_name>"
            ))
        }
    }
}

impl Display for WorkerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.slug())
    }
}

impl From<WorkerId> for golem_api_grpc::proto::golem::worker::WorkerId {
    fn from(value: WorkerId) -> Self {
        Self {
            template_id: Some(value.template_id.into()),
            name: value.worker_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerId> for WorkerId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            template_id: value.template_id.unwrap().try_into()?,
            worker_name: value.name,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct PromiseId {
    #[serde(rename = "instance_id")]
    pub worker_id: WorkerId,
    pub oplog_idx: i32,
}

impl PromiseId {
    pub fn from_json_string(s: &str) -> PromiseId {
        serde_json::from_str(s)
            .unwrap_or_else(|err| panic!("failed to deserialize promise id: {s}: {err}"))
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|err| panic!("failed to serialize promise id {self}: {err}"))
    }

    pub fn to_redis_key(&self) -> String {
        format!("{}:{}", self.worker_id.to_redis_key(), self.oplog_idx)
    }
}

impl From<PromiseId> for golem_api_grpc::proto::golem::worker::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            oplog_idx: value.oplog_idx,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PromiseId> for PromiseId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PromiseId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            oplog_idx: value.oplog_idx,
        })
    }
}

impl Display for PromiseId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.worker_id, self.oplog_idx)
    }
}

#[derive(Debug, Serialize, Deserialize, Encode, Decode)]
pub struct ScheduleId {
    pub timestamp: i64,
    pub promise_id: PromiseId,
}

impl Display for ScheduleId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.promise_id, self.timestamp)
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Encode,
    Decode,
    Object,
)]
pub struct ShardId {
    value: i64,
}

impl ShardId {
    pub fn new(value: i64) -> Self {
        Self { value }
    }

    pub fn from_worker_id(worker_id: &WorkerId, number_of_shards: usize) -> Self {
        let hash = Self::hash_worker_id(worker_id);
        let value = hash.abs() % number_of_shards as i64;
        Self { value }
    }

    pub fn hash_worker_id(worker_id: &WorkerId) -> i64 {
        let (high_bits, low_bits) = (
            (worker_id.template_id.0.as_u128() >> 64) as i64,
            worker_id.template_id.0.as_u128() as i64,
        );
        let high = Self::hash_string(&high_bits.to_string());
        let worker_name = &worker_id.worker_name;
        let template_worker_name = format!("{}{}", low_bits, worker_name);
        let low = Self::hash_string(&template_worker_name);
        ((high as i64) << 32) | ((low as i64) & 0xFFFFFFFF)
    }

    fn hash_string(string: &str) -> i32 {
        let mut hash = 0;
        if hash == 0 && !string.is_empty() {
            for val in &mut string.bytes() {
                hash = 31_i32.wrapping_mul(hash).wrapping_add(val as i32);
            }
        }
        hash
    }

    pub fn is_left_neighbor(&self, other: &ShardId) -> bool {
        other.value == self.value + 1
    }
}

impl Display for ShardId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{}>", self.value)
    }
}

impl From<ShardId> for golem_api_grpc::proto::golem::shardmanager::ShardId {
    fn from(value: ShardId) -> golem_api_grpc::proto::golem::shardmanager::ShardId {
        golem_api_grpc::proto::golem::shardmanager::ShardId { value: value.value }
    }
}

impl From<golem_api_grpc::proto::golem::shardmanager::ShardId> for ShardId {
    fn from(proto: golem_api_grpc::proto::golem::shardmanager::ShardId) -> Self {
        Self { value: proto.value }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ShardAssignment {
    pub number_of_shards: usize,
    pub shard_ids: HashSet<ShardId>,
}

impl ShardAssignment {
    pub fn new(number_of_shards: usize, shard_ids: HashSet<ShardId>) -> Self {
        Self {
            number_of_shards,
            shard_ids,
        }
    }

    pub fn assign_shards(&mut self, shard_ids: &HashSet<ShardId>) {
        for shard_id in shard_ids {
            self.shard_ids.insert(*shard_id);
        }
    }

    pub fn register(&mut self, number_of_shards: usize, shard_ids: &HashSet<ShardId>) {
        self.number_of_shards = number_of_shards;
        for shard_id in shard_ids {
            self.shard_ids.insert(*shard_id);
        }
    }

    pub fn revoke_shards(&mut self, shard_ids: &HashSet<ShardId>) {
        for shard_id in shard_ids {
            self.shard_ids.remove(shard_id);
        }
    }
}

impl Display for ShardAssignment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let shard_ids = self
            .shard_ids
            .iter()
            .map(|shard_id| shard_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        write!(
            f,
            "{{ number_of_shards: {}, shard_ids: {} }}",
            self.number_of_shards, shard_ids
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode, Eq, Hash, PartialEq, Object)]
pub struct InvocationKey {
    pub value: String,
}

impl InvocationKey {
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

impl From<golem_api_grpc::proto::golem::worker::InvocationKey> for InvocationKey {
    fn from(proto: golem_api_grpc::proto::golem::worker::InvocationKey) -> Self {
        Self { value: proto.value }
    }
}

impl From<InvocationKey> for golem_api_grpc::proto::golem::worker::InvocationKey {
    fn from(value: InvocationKey) -> Self {
        Self { value: value.value }
    }
}

impl Display for InvocationKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode, Enum)]
pub enum CallingConvention {
    Component,
    Stdio,
    StdioEventloop,
}

impl TryFrom<i32> for CallingConvention {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(CallingConvention::Component),
            1 => Ok(CallingConvention::Stdio),
            2 => Ok(CallingConvention::StdioEventloop),
            _ => Err(format!("Unknown calling convention: {}", value)),
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::CallingConvention> for CallingConvention {
    fn from(value: golem_api_grpc::proto::golem::worker::CallingConvention) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::CallingConvention::Component => {
                CallingConvention::Component
            }
            golem_api_grpc::proto::golem::worker::CallingConvention::Stdio => {
                CallingConvention::Stdio
            }
            golem_api_grpc::proto::golem::worker::CallingConvention::StdioEventloop => {
                CallingConvention::StdioEventloop
            }
        }
    }
}

impl From<CallingConvention> for i32 {
    fn from(value: CallingConvention) -> Self {
        match value {
            CallingConvention::Component => 0,
            CallingConvention::Stdio => 1,
            CallingConvention::StdioEventloop => 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct WorkerMetadata {
    #[serde(rename = "instance_id")]
    pub worker_id: VersionedWorkerId,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub account_id: AccountId,
    #[serde(skip)]
    pub last_known_status: WorkerStatusRecord, // serialized separately
}

impl WorkerMetadata {
    #[allow(unused)] // TODO
    pub fn default(worker_id: VersionedWorkerId, account_id: AccountId) -> WorkerMetadata {
        WorkerMetadata {
            worker_id,
            args: vec![],
            env: vec![],
            account_id,
            last_known_status: WorkerStatusRecord::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct WorkerStatusRecord {
    pub status: WorkerStatus,
    pub oplog_idx: i32,
}

impl Default for WorkerStatusRecord {
    fn default() -> Self {
        WorkerStatusRecord {
            status: WorkerStatus::Idle,
            oplog_idx: 0,
        }
    }
}

/// Represents last known status of a worker
///
/// This is always recorded together with the current oplog index, and it can only be used
/// as a source of truth if there are no newer oplog entries since the record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Enum)]
pub enum WorkerStatus {
    /// The worker is running an invoked function
    Running,
    /// The worker is ready to run an invoked function
    Idle,
    /// An invocation is active but waiting for something (sleeping, waiting for a promise)
    Suspended,
    /// The last invocation was interrupted but will be resumed
    Interrupted,
    /// The last invocation failed an a retry was scheduled
    Retrying,
    /// The last invocation failed and the worker can no longer be used
    Failed,
    /// The worker exited after a successful invocation and can no longer be invoked
    Exited,
}

impl From<WorkerStatus> for golem_api_grpc::proto::golem::worker::WorkerStatus {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => golem_api_grpc::proto::golem::worker::WorkerStatus::Running,
            WorkerStatus::Idle => golem_api_grpc::proto::golem::worker::WorkerStatus::Idle,
            WorkerStatus::Suspended => {
                golem_api_grpc::proto::golem::worker::WorkerStatus::Suspended
            }
            WorkerStatus::Interrupted => {
                golem_api_grpc::proto::golem::worker::WorkerStatus::Interrupted
            }
            WorkerStatus::Retrying => golem_api_grpc::proto::golem::worker::WorkerStatus::Retrying,
            WorkerStatus::Failed => golem_api_grpc::proto::golem::worker::WorkerStatus::Failed,
            WorkerStatus::Exited => golem_api_grpc::proto::golem::worker::WorkerStatus::Exited,
        }
    }
}

impl TryFrom<i32> for WorkerStatus {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(WorkerStatus::Running),
            1 => Ok(WorkerStatus::Idle),
            2 => Ok(WorkerStatus::Suspended),
            3 => Ok(WorkerStatus::Interrupted),
            4 => Ok(WorkerStatus::Retrying),
            5 => Ok(WorkerStatus::Failed),
            6 => Ok(WorkerStatus::Exited),
            _ => Err(format!("Unknown worker status: {}", value)),
        }
    }
}

impl From<WorkerStatus> for i32 {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => 0,
            WorkerStatus::Idle => 1,
            WorkerStatus::Suspended => 2,
            WorkerStatus::Interrupted => 3,
            WorkerStatus::Retrying => 4,
            WorkerStatus::Failed => 5,
            WorkerStatus::Exited => 6,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerMetadata> for WorkerMetadata {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerMetadata,
    ) -> Result<Self, Self::Error> {
        let worker_id: WorkerId = value.worker_id.ok_or("Missing worker_id")?.try_into()?;
        Ok(Self {
            worker_id: VersionedWorkerId {
                worker_id,
                template_version: value.template_version,
            },
            args: value.args,
            env: value.env.into_iter().collect(),
            account_id: value.account_id.ok_or("Missing account_id")?.into(),
            last_known_status: WorkerStatusRecord {
                status: value.status.try_into()?,
                oplog_idx: -1,
            },
        })
    }
}

#[derive(
    Clone,
    Debug,
    PartialOrd,
    Ord,
    FromStr,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
    Encode,
    Decode,
)]
#[serde(transparent)]
pub struct AccountId {
    pub value: String,
}

impl AccountId {
    pub fn generate() -> Self {
        Self {
            value: Uuid::new_v4().to_string(),
        }
    }
}

impl From<&str> for AccountId {
    fn from(value: &str) -> Self {
        Self {
            value: value.to_string(),
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::AccountId> for AccountId {
    fn from(proto: golem_api_grpc::proto::golem::common::AccountId) -> Self {
        Self { value: proto.name }
    }
}

impl From<AccountId> for golem_api_grpc::proto::golem::common::AccountId {
    fn from(value: AccountId) -> Self {
        golem_api_grpc::proto::golem::common::AccountId { name: value.value }
    }
}

impl poem_openapi::types::Type for AccountId {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        Cow::from("string(account_id)")
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new("string")))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl ParseFromParameter for AccountId {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        Ok(Self {
            value: value.to_string(),
        })
    }
}

impl ParseFromJSON for AccountId {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(Value::String(s)) => Ok(Self { value: s }),
            _ => Err(poem_openapi::types::ParseError::<AccountId>::custom(
                "Unexpected representation of AccountId".to_string(),
            )),
        }
    }
}

impl ToJSON for AccountId {
    fn to_json(&self) -> Option<Value> {
        Some(Value::String(self.value.clone()))
    }
}

impl Display for AccountId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum OplogEntry {
    ImportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        response: Vec<u8>,
        wrapped_function_type: WrappedFunctionType,
    },
    ExportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        request: Vec<u8>,
        invocation_key: Option<InvocationKey>,
        calling_convention: Option<CallingConvention>,
    },
    ExportedFunctionCompleted {
        timestamp: Timestamp,
        response: Vec<u8>,
        consumed_fuel: i64,
    },
    CreatePromise {
        timestamp: Timestamp,
        promise_id: PromiseId,
    },
    CompletePromise {
        timestamp: Timestamp,
        promise_id: PromiseId,
        data: Vec<u8>,
    },
    Suspend {
        timestamp: Timestamp,
    },
    Error {
        timestamp: Timestamp,
    },
    Debug {
        timestamp: Timestamp,
        message: String,
    },
}

impl OplogEntry {
    pub fn imported_function_invoked<R: Encode>(
        timestamp: Timestamp,
        function_name: String,
        response: &R,
        wrapped_function_type: WrappedFunctionType,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();

        Ok(OplogEntry::ImportedFunctionInvoked {
            timestamp,
            function_name,
            response: serialized_response,
            wrapped_function_type,
        })
    }

    pub fn exported_function_invoked<R: Encode>(
        timestamp: Timestamp,
        function_name: String,
        request: &R,
        invocation_key: Option<InvocationKey>,
        calling_convention: Option<CallingConvention>,
    ) -> Result<OplogEntry, String> {
        let serialized_request = serialize(request)?.to_vec();
        Ok(OplogEntry::ExportedFunctionInvoked {
            timestamp,
            function_name,
            request: serialized_request,
            invocation_key,
            calling_convention,
        })
    }

    pub fn exported_function_completed<R: Encode>(
        timestamp: Timestamp,
        response: &R,
        consumed_fuel: i64,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();
        Ok(OplogEntry::ExportedFunctionCompleted {
            timestamp,
            response: serialized_response,
            consumed_fuel,
        })
    }

    pub fn response<T: DeserializeOwned + Decode>(&self) -> Result<Option<T>, String> {
        match &self {
            OplogEntry::ImportedFunctionInvoked { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);

                // In the v1 serialization format we did not have version prefix in the payloads.
                // We can assume though that if the payload starts with 2, it is serialized with the
                // v2 format because neither JSON nor protobuf (the two payload formats used in v1 for payloads)
                // start with 2 (This was verified with a simple test ValProtobufPrefixByteValidation).
                // So if the first byte is not 1 or 2 we assume it is a v1 format and deserialize it as JSON.
                match try_deserialize(&response_bytes)? {
                    Some(result) => Ok(Some(result)),
                    None => Ok(Some(deserialize_with_version(
                        &response_bytes,
                        SERIALIZATION_VERSION_V1,
                    )?)),
                }
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);

                // See the comment above for the explanation of this logic
                match try_deserialize(&response_bytes)? {
                    Some(result) => Ok(Some(result)),
                    None => Ok(Some(deserialize_with_version(
                        &response_bytes,
                        SERIALIZATION_VERSION_V1,
                    )?)),
                }
            }
            _ => Ok(None),
        }
    }

    pub fn payload_as_val_array(
        &self,
    ) -> Result<Option<Vec<golem_api_grpc::proto::golem::worker::Val>>, String> {
        // This is a special case of a possible generic request() accessor, because in v1 the only
        // data type we serialized was Vec<Val> and it was done in a special way (every element serialized
        // via protobuf separately, then an array of byte arrays serialized into JSON)
        match &self {
            OplogEntry::ExportedFunctionInvoked {
                function_name,
                request,
                ..
            } => {
                let request_bytes: Bytes = Bytes::copy_from_slice(request);
                self.try_decode_val_array_payload(function_name, &request_bytes)
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);
                self.try_decode_val_array_payload("?", &response_bytes)
            }
            _ => Ok(None),
        }
    }

    fn try_decode_val_array_payload(
        &self,
        function_name: &str,
        payload: &Bytes,
    ) -> Result<Option<Vec<golem_api_grpc::proto::golem::worker::Val>>, String> {
        match try_deserialize(payload)? {
            Some(result) => Ok(Some(result)),
            None => {
                let deserialized_array: Vec<Vec<u8>> = serde_json::from_slice(payload)
                    .unwrap_or_else(|err| {
                        panic!(
                            "Failed to deserialize oplog payload: {:?}: {err}",
                            std::str::from_utf8(payload).unwrap_or("???")
                        )
                    });
                let function_input = deserialized_array
                    .iter()
                    .map(|serialized_value| {
                        <golem_api_grpc::proto::golem::worker::Val as prost::Message>::decode(serialized_value.as_slice())
                            .unwrap_or_else(|err| panic!("Failed to deserialize function input {:?} for {function_name}: {err}", serialized_value))
                    })
                    .collect::<Vec<golem_api_grpc::proto::golem::worker::Val>>();
                Ok(Some(function_input))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum WrappedFunctionType {
    ReadLocal,
    WriteLocal,
    ReadRemote,
    WriteRemote,
}

#[cfg(test)]
mod tests {
    use crate::model::AccountId;
    use crate::model::{CallingConvention, InvocationKey, Timestamp};
    use crate::model::{OplogEntry, WrappedFunctionType};
    use bincode::{Decode, Encode};
    use golem_api_grpc::proto::golem::worker::{val, Val, ValResult};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
    struct ExampleWithAccountId {
        account_id: AccountId,
    }

    #[test]
    fn account_id_from_json_apigateway_version() {
        let json = "{ \"account_id\": \"account-1\" }";
        let example: ExampleWithAccountId = serde_json::from_str(json).unwrap();
        assert_eq!(
            example.account_id,
            AccountId {
                value: "account-1".to_string()
            }
        );
    }

    #[test]
    fn account_id_json_serialization() {
        // We want to use this variant for serialization because it is used on the public API gateway API
        let example: ExampleWithAccountId = ExampleWithAccountId {
            account_id: AccountId {
                value: "account-1".to_string(),
            },
        };
        let json = serde_json::to_string(&example).unwrap();
        assert_eq!(json, "{\"account_id\":\"account-1\"}");
    }

    #[test]
    fn oplog_entry_imported_function_invoked_payload_roundtrip() {
        let timestamp = Timestamp::now_utc();
        let entry = OplogEntry::imported_function_invoked(
            timestamp,
            "function_name".to_string(),
            &("example payload".to_string()),
            WrappedFunctionType::ReadLocal,
        )
        .unwrap();

        if let OplogEntry::ImportedFunctionInvoked { response, .. } = &entry {
            assert_eq!(response.len(), 17);
        } else {
            unreachable!()
        }

        let response = entry.response::<String>().unwrap().unwrap();

        assert_eq!(response, "example payload");
    }

    #[test]
    fn oplog_entry_imported_function_invoked_payload_v1() {
        let timestamp = Timestamp::now_utc();
        let entry = OplogEntry::ImportedFunctionInvoked {
            timestamp,
            function_name: "function_name".to_string(),
            response: serde_json::to_vec("example payload").unwrap(),
            wrapped_function_type: WrappedFunctionType::ReadLocal,
        };

        let response = entry.response::<String>().unwrap().unwrap();

        assert_eq!(response, "example payload");
    }

    #[test]
    fn oplog_entry_exported_function_invoked_payload_roundtrip() {
        let timestamp = Timestamp::now_utc();

        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let entry = OplogEntry::exported_function_invoked(
            timestamp,
            "function_name".to_string(),
            &vec![val1.clone()],
            Some(InvocationKey {
                value: "invocation_key".to_string(),
            }),
            Some(CallingConvention::Stdio),
        )
        .unwrap();

        if let OplogEntry::ExportedFunctionInvoked { request, .. } = &entry {
            assert_eq!(request.len(), 9);
        } else {
            unreachable!()
        }

        let request = entry.payload_as_val_array().unwrap().unwrap();

        assert_eq!(request, vec![val1]);
    }

    #[test]
    fn oplog_entry_exported_function_invoked_payload_v1() {
        let timestamp = Timestamp::now_utc();

        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let val1_bytes = prost::Message::encode_to_vec(&val1);
        let request_bytes = serde_json::to_vec(&vec![val1_bytes]).unwrap();

        let entry = OplogEntry::ExportedFunctionInvoked {
            timestamp,
            function_name: "function_name".to_string(),
            request: request_bytes,
            invocation_key: Some(InvocationKey {
                value: "invocation_key".to_string(),
            }),
            calling_convention: Some(CallingConvention::Stdio),
        };

        let request = entry.payload_as_val_array().unwrap().unwrap();
        assert_eq!(request, vec![val1]);
    }

    #[test]
    fn oplog_entry_exported_function_completed_roundtrip() {
        let timestamp = Timestamp::now_utc();

        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let val2 = Val {
            val: Some(val::Val::String("something".to_string())),
        };

        let entry = OplogEntry::exported_function_completed(
            timestamp,
            &vec![val1.clone(), val2.clone()],
            1_000_000_000,
        )
        .unwrap();

        if let OplogEntry::ExportedFunctionCompleted { response, .. } = &entry {
            assert_eq!(response.len(), 21);
        } else {
            unreachable!()
        }

        let response = entry.payload_as_val_array().unwrap().unwrap();

        assert_eq!(response, vec![val1, val2]);
    }

    #[test]
    fn oplog_entry_exported_function_completed_v1() {
        let timestamp = Timestamp::now_utc();

        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let val1_bytes = prost::Message::encode_to_vec(&val1);
        let val2 = Val {
            val: Some(val::Val::String("something".to_string())),
        };
        let val2_bytes = prost::Message::encode_to_vec(&val2);

        let response_bytes = serde_json::to_vec(&vec![val1_bytes, val2_bytes]).unwrap();

        let entry = OplogEntry::ExportedFunctionCompleted {
            timestamp,
            response: response_bytes,
            consumed_fuel: 1_000_000_000,
        };

        let response = entry.payload_as_val_array().unwrap().unwrap();

        assert_eq!(response, vec![val1, val2]);
    }
}
