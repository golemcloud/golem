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
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
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
use derive_more::FromStr;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ParseFromParameter, ParseResult, ToJSON};
use poem_openapi::{Enum, Object, Union};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

use crate::config::RetryConfig;
use crate::model::oplog::{OplogIndex, TimestampedUpdateDescription};
use crate::model::regions::DeletedRegions;
use crate::newtype_uuid;

pub mod oplog;
pub mod regions;

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

impl ToJSON for Timestamp {
    fn to_json(&self) -> Option<Value> {
        Some(Value::String(self.0.to_string()))
    }
}

impl poem_openapi::types::Type for Timestamp {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        Cow::from("string(timestamp)")
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format("string", "date-time")))
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

impl ParseFromParameter for Timestamp {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        value.parse().map_err(|_| {
            poem_openapi::types::ParseError::<Timestamp>::custom(
                "Unexpected representation of timestamp".to_string(),
            )
        })
    }
}

impl ParseFromJSON for Timestamp {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(Value::String(s)) => Timestamp::parse_from_parameter(&s),
            _ => Err(poem_openapi::types::ParseError::<Timestamp>::custom(
                "Unexpected representation of timestamp".to_string(),
            )),
        }
    }
}

impl FromStr for Timestamp {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match iso8601_timestamp::Timestamp::parse(s) {
            Some(ts) => Ok(Self(ts)),
            None => Err("Invalid timestamp".to_string()),
        }
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

impl From<Timestamp> for prost_types::Timestamp {
    fn from(value: Timestamp) -> Self {
        let d = value
            .0
            .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH);
        Self {
            seconds: d.whole_seconds(),
            nanos: d.subsec_nanoseconds(),
        }
    }
}

impl From<prost_types::Timestamp> for Timestamp {
    fn from(value: prost_types::Timestamp) -> Self {
        Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH
                .add(Duration::new(value.seconds as u64, value.nanos as u32)),
        )
    }
}

impl From<u64> for Timestamp {
    fn from(value: u64) -> Self {
        Timestamp(iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(value)))
    }
}

pub type ComponentVersion = u64;

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
    pub oplog_idx: OplogIndex,
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode, Enum)]
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

#[derive(Clone, Debug)]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub account_id: AccountId,
    pub created_at: Timestamp,
    pub last_known_status: WorkerStatusRecord,
}

impl WorkerMetadata {
    pub fn default(worker_id: WorkerId, account_id: AccountId) -> WorkerMetadata {
        WorkerMetadata {
            worker_id,
            args: vec![],
            env: vec![],
            account_id,
            created_at: Timestamp::now_utc(),
            last_known_status: WorkerStatusRecord::default(),
        }
    }
}

/// Contains status information about a worker according to a given oplog index.
/// This status is just cached information, all fields must be computable by the oplog alone.
/// By having an associated oplog_idx, the cached information can be used together with the
/// tail of the oplog to determine the actual status of the worker.
#[derive(Clone, Debug, Encode, Decode)]
pub struct WorkerStatusRecord {
    pub status: WorkerStatus,
    pub deleted_regions: DeletedRegions,
    pub overridden_retry_config: Option<RetryConfig>,
    pub pending_invocations: Vec<TimestampedWorkerInvocation>,
    pub pending_updates: VecDeque<TimestampedUpdateDescription>,
    pub failed_updates: Vec<FailedUpdateRecord>,
    pub successful_updates: Vec<SuccessfulUpdateRecord>,
    pub component_version: ComponentVersion,
    pub oplog_idx: OplogIndex,
}

impl Default for WorkerStatusRecord {
    fn default() -> Self {
        WorkerStatusRecord {
            status: WorkerStatus::Idle,
            deleted_regions: DeletedRegions::new(),
            overridden_retry_config: None,
            pending_invocations: Vec::new(),
            pending_updates: VecDeque::new(),
            failed_updates: Vec::new(),
            successful_updates: Vec::new(),
            component_version: 0,
            oplog_idx: 0,
        }
    }
}

#[derive(Clone, Debug, Encode, Decode)]
pub struct FailedUpdateRecord {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub details: Option<String>,
}

#[derive(Clone, Debug, Encode, Decode)]
pub struct SuccessfulUpdateRecord {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
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
    /// The last invocation failed and a retry was scheduled
    Retrying,
    /// The last invocation failed and the worker can no longer be used
    Failed,
    /// The worker exited after a successful invocation and can no longer be invoked
    Exited,
}

impl PartialOrd for WorkerStatus {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WorkerStatus {
    fn cmp(&self, other: &Self) -> Ordering {
        let v1: i32 = self.clone().into();
        let v2: i32 = other.clone().into();
        v1.cmp(&v2)
    }
}

impl FromStr for WorkerStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "running" => Ok(WorkerStatus::Running),
            "idle" => Ok(WorkerStatus::Idle),
            "suspended" => Ok(WorkerStatus::Suspended),
            "interrupted" => Ok(WorkerStatus::Interrupted),
            "retrying" => Ok(WorkerStatus::Retrying),
            "failed" => Ok(WorkerStatus::Failed),
            "exited" => Ok(WorkerStatus::Exited),
            _ => Err(format!("Unknown worker status: {}", s)),
        }
    }
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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum WorkerInvocation {
    ExportedFunction {
        invocation_key: InvocationKey,
        full_function_name: String,
        function_input: Vec<golem_wasm_rpc::Value>,
        calling_convention: CallingConvention,
    },
    ManualUpdate {
        target_version: ComponentVersion,
    },
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct TimestampedWorkerInvocation {
    pub timestamp: Timestamp,
    pub invocation: WorkerInvocation,
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

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ParsedFunctionName {
    pub interface: Option<String>,
    pub function: String,
}

impl ParsedFunctionName {
    pub fn new(interface: Option<String>, function: String) -> Self {
        Self {
            interface,
            function,
        }
    }

    pub fn method_as_static(&self) -> Option<ParsedFunctionName> {
        if self.function.starts_with("[method]") {
            Some(ParsedFunctionName {
                interface: self.interface.clone(),
                function: self.function.replace("[method]", "[static]"),
            })
        } else {
            None
        }
    }
}

pub fn parse_function_name(name: &str) -> ParsedFunctionName {
    let parts = name.match_indices('/').collect::<Vec<_>>();
    match parts.len() {
        1 => ParsedFunctionName::new(
            Some(name[0..parts[0].0].to_string()),
            name[(parts[0].0 + 1)..name.len()].to_string(),
        ),
        2 => ParsedFunctionName::new(
            Some(name[0..parts[1].0].to_string()),
            name[(parts[1].0 + 1)..name.len()].to_string(),
        ),
        3 => {
            let instance = &name[0..parts[1].0];
            let resource_name = &name[(parts[1].0 + 1)..parts[2].0];
            let function_name = &name[(parts[2].0 + 1)..name.len()];

            match function_name {
                "new" => ParsedFunctionName::new(
                    Some(instance.to_string()),
                    format!("[constructor]{}", resource_name),
                ),
                "drop" => ParsedFunctionName::new(
                    Some(instance.to_string()),
                    format!("[drop]{}", resource_name),
                ),
                _ => ParsedFunctionName::new(
                    Some(instance.to_string()),
                    format!("[method]{}.{}", resource_name, function_name),
                ),
            }
        }
        _ => ParsedFunctionName::new(None, name.to_string()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerNameFilter {
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl WorkerNameFilter {
    pub fn new(comparator: StringFilterComparator, value: String) -> Self {
        Self { comparator, value }
    }
}

impl Display for WorkerNameFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "name {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerStatusFilter {
    pub comparator: FilterComparator,
    pub value: WorkerStatus,
}

impl WorkerStatusFilter {
    pub fn new(comparator: FilterComparator, value: WorkerStatus) -> Self {
        Self { comparator, value }
    }
}

impl Display for WorkerStatusFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "status == {:?}", self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerVersionFilter {
    pub comparator: FilterComparator,
    pub value: ComponentVersion,
}

impl WorkerVersionFilter {
    pub fn new(comparator: FilterComparator, value: ComponentVersion) -> Self {
        Self { comparator, value }
    }
}

impl Display for WorkerVersionFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "version {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerCreatedAtFilter {
    pub comparator: FilterComparator,
    pub value: Timestamp,
}

impl WorkerCreatedAtFilter {
    pub fn new(comparator: FilterComparator, value: Timestamp) -> Self {
        Self { comparator, value }
    }
}

impl Display for WorkerCreatedAtFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "created_at {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerEnvFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl WorkerEnvFilter {
    pub fn new(name: String, comparator: StringFilterComparator, value: String) -> Self {
        Self {
            name,
            comparator,
            value,
        }
    }
}

impl Display for WorkerEnvFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "env.{} {} {}", self.name, self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerAndFilter {
    pub filters: Vec<WorkerFilter>,
}

impl WorkerAndFilter {
    pub fn new(filters: Vec<WorkerFilter>) -> Self {
        Self { filters }
    }
}

impl Display for WorkerAndFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({})",
            self.filters
                .iter()
                .map(|f| f.clone().to_string())
                .collect::<Vec<String>>()
                .join(" AND ")
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerOrFilter {
    pub filters: Vec<WorkerFilter>,
}

impl WorkerOrFilter {
    pub fn new(filters: Vec<WorkerFilter>) -> Self {
        Self { filters }
    }
}

impl Display for WorkerOrFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({})",
            self.filters
                .iter()
                .map(|f| f.clone().to_string())
                .collect::<Vec<String>>()
                .join(" OR ")
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Object)]
pub struct WorkerNotFilter {
    filter: Box<WorkerFilter>,
}

impl WorkerNotFilter {
    pub fn new(filter: WorkerFilter) -> Self {
        Self {
            filter: Box::new(filter),
        }
    }
}

impl Display for WorkerNotFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NOT ({})", self.filter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum WorkerFilter {
    Name(WorkerNameFilter),
    Status(WorkerStatusFilter),
    Version(WorkerVersionFilter),
    CreatedAt(WorkerCreatedAtFilter),
    Env(WorkerEnvFilter),
    And(WorkerAndFilter),
    Or(WorkerOrFilter),
    Not(WorkerNotFilter),
}

impl WorkerFilter {
    pub fn and(&self, filter: WorkerFilter) -> Self {
        match self.clone() {
            WorkerFilter::And(WorkerAndFilter { filters }) => {
                Self::new_and([filters, vec![filter]].concat())
            }
            f => Self::new_and(vec![f, filter]),
        }
    }

    pub fn or(&self, filter: WorkerFilter) -> Self {
        match self.clone() {
            WorkerFilter::Or(WorkerOrFilter { filters }) => {
                Self::new_or([filters, vec![filter]].concat())
            }
            f => Self::new_or(vec![f, filter]),
        }
    }

    pub fn not(&self) -> Self {
        Self::new_not(self.clone())
    }

    pub fn matches(&self, metadata: &WorkerMetadata) -> bool {
        match self.clone() {
            WorkerFilter::Name(WorkerNameFilter { comparator, value }) => {
                comparator.matches(&metadata.worker_id.worker_name, &value)
            }
            WorkerFilter::Version(WorkerVersionFilter { comparator, value }) => {
                let version: ComponentVersion = metadata.last_known_status.component_version;
                comparator.matches(&version, &value)
            }
            WorkerFilter::Env(WorkerEnvFilter {
                name,
                comparator,
                value,
            }) => {
                let mut result = false;
                let name = name.to_lowercase();
                for env_value in metadata.env.clone() {
                    if env_value.0.to_lowercase() == name {
                        result = comparator.matches(&env_value.1, &value);

                        break;
                    }
                }
                result
            }
            WorkerFilter::CreatedAt(WorkerCreatedAtFilter { comparator, value }) => {
                comparator.matches(&metadata.created_at, &value)
            }
            WorkerFilter::Status(WorkerStatusFilter { comparator, value }) => {
                comparator.matches(&metadata.last_known_status.status, &value)
            }
            WorkerFilter::Not(WorkerNotFilter { filter }) => !filter.matches(metadata),
            WorkerFilter::And(WorkerAndFilter { filters }) => {
                let mut result = true;
                for filter in filters {
                    if !filter.matches(metadata) {
                        result = false;
                        break;
                    }
                }
                result
            }
            WorkerFilter::Or(WorkerOrFilter { filters }) => {
                let mut result = true;
                if !filters.is_empty() {
                    result = false;
                    for filter in filters {
                        if filter.matches(metadata) {
                            result = true;
                            break;
                        }
                    }
                }
                result
            }
        }
    }

    pub fn new_and(filters: Vec<WorkerFilter>) -> Self {
        WorkerFilter::And(WorkerAndFilter::new(filters))
    }

    pub fn new_or(filters: Vec<WorkerFilter>) -> Self {
        WorkerFilter::Or(WorkerOrFilter::new(filters))
    }

    pub fn new_not(filter: WorkerFilter) -> Self {
        WorkerFilter::Not(WorkerNotFilter::new(filter))
    }

    pub fn new_name(comparator: StringFilterComparator, value: String) -> Self {
        WorkerFilter::Name(WorkerNameFilter::new(comparator, value))
    }

    pub fn new_env(name: String, comparator: StringFilterComparator, value: String) -> Self {
        WorkerFilter::Env(WorkerEnvFilter::new(name, comparator, value))
    }

    pub fn new_version(comparator: FilterComparator, value: ComponentVersion) -> Self {
        WorkerFilter::Version(WorkerVersionFilter::new(comparator, value))
    }

    pub fn new_status(comparator: FilterComparator, value: WorkerStatus) -> Self {
        WorkerFilter::Status(WorkerStatusFilter::new(comparator, value))
    }

    pub fn new_created_at(comparator: FilterComparator, value: Timestamp) -> Self {
        WorkerFilter::CreatedAt(WorkerCreatedAtFilter::new(comparator, value))
    }

    pub fn from(filters: Vec<String>) -> Result<WorkerFilter, String> {
        let mut fs = Vec::new();
        for f in filters {
            fs.push(WorkerFilter::from_str(&f)?);
        }
        Ok(WorkerFilter::new_and(fs))
    }
}

impl Display for WorkerFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerFilter::Name(filter) => {
                write!(f, "{}", filter)
            }
            WorkerFilter::Version(filter) => {
                write!(f, "{}", filter)
            }
            WorkerFilter::Status(filter) => {
                write!(f, "{}", filter)
            }
            WorkerFilter::CreatedAt(filter) => {
                write!(f, "{}", filter)
            }
            WorkerFilter::Env(filter) => {
                write!(f, "{}", filter)
            }
            WorkerFilter::Not(filter) => {
                write!(f, "{}", filter)
            }
            WorkerFilter::And(filter) => {
                write!(f, "{}", filter)
            }
            WorkerFilter::Or(filter) => {
                write!(f, "{}", filter)
            }
        }
    }
}

impl FromStr for WorkerFilter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let elements = s.split_whitespace().collect::<Vec<&str>>();

        if elements.len() == 3 {
            let arg = elements[0];
            let comparator = elements[1];
            let value = elements[2];
            match arg {
                "name" => Ok(WorkerFilter::new_name(
                    comparator.parse()?,
                    value.to_string(),
                )),
                "version" => Ok(WorkerFilter::new_version(
                    comparator.parse()?,
                    value
                        .parse()
                        .map_err(|e| format!("Invalid filter value: {}", e))?,
                )),
                "status" => Ok(WorkerFilter::new_status(
                    comparator.parse()?,
                    value.parse()?,
                )),
                "created_at" | "createdAt" => Ok(WorkerFilter::new_created_at(
                    comparator.parse()?,
                    value.parse()?,
                )),
                _ if arg.starts_with("env.") => {
                    let name = &arg[4..];
                    Ok(WorkerFilter::new_env(
                        name.to_string(),
                        comparator.parse()?,
                        value.to_string(),
                    ))
                }
                _ => Err(format!("Invalid filter: {}", s)),
            }
        } else {
            Err(format!("Invalid filter: {}", s))
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerFilter> for WorkerFilter {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerFilter,
    ) -> Result<Self, Self::Error> {
        match value.filter {
            Some(filter) => match filter {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Name(filter) => Ok(
                    WorkerFilter::new_name(filter.comparator.try_into()?, filter.value),
                ),
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Version(filter) => Ok(
                    WorkerFilter::new_version(filter.comparator.try_into()?, filter.value),
                ),
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Status(filter) => {
                    Ok(WorkerFilter::new_status(
                        filter.comparator.try_into()?,
                        filter.value.try_into()?,
                    ))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::CreatedAt(filter) => {
                    let value = filter
                        .value
                        .map(|t| t.into())
                        .ok_or_else(|| "Missing value".to_string())?;
                    Ok(WorkerFilter::new_created_at(
                        filter.comparator.try_into()?,
                        value,
                    ))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Env(filter) => Ok(
                    WorkerFilter::new_env(filter.name, filter.comparator.try_into()?, filter.value),
                ),
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Not(filter) => {
                    let filter = *filter.filter.ok_or_else(|| "Missing filter".to_string())?;
                    Ok(WorkerFilter::new_not(filter.try_into()?))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::And(
                    golem_api_grpc::proto::golem::worker::WorkerAndFilter { filters },
                ) => {
                    let filters = filters.into_iter().map(|f| f.try_into()).collect::<Result<
                        Vec<WorkerFilter>,
                        String,
                    >>(
                    )?;

                    Ok(WorkerFilter::new_and(filters))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Or(
                    golem_api_grpc::proto::golem::worker::WorkerOrFilter { filters },
                ) => {
                    let filters = filters.into_iter().map(|f| f.try_into()).collect::<Result<
                        Vec<WorkerFilter>,
                        String,
                    >>(
                    )?;

                    Ok(WorkerFilter::new_or(filters))
                }
            },
            None => Err("Missing filter".to_string()),
        }
    }
}

impl From<WorkerFilter> for golem_api_grpc::proto::golem::worker::WorkerFilter {
    fn from(value: WorkerFilter) -> Self {
        let filter = match value {
            WorkerFilter::Name(WorkerNameFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Name(
                    golem_api_grpc::proto::golem::worker::WorkerNameFilter {
                        comparator: comparator.into(),
                        value,
                    },
                )
            }
            WorkerFilter::Version(WorkerVersionFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Version(
                    golem_api_grpc::proto::golem::worker::WorkerVersionFilter {
                        comparator: comparator.into(),
                        value,
                    },
                )
            }
            WorkerFilter::Env(WorkerEnvFilter {
                name,
                comparator,
                value,
            }) => golem_api_grpc::proto::golem::worker::worker_filter::Filter::Env(
                golem_api_grpc::proto::golem::worker::WorkerEnvFilter {
                    name,
                    comparator: comparator.into(),
                    value,
                },
            ),
            WorkerFilter::Status(WorkerStatusFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Status(
                    golem_api_grpc::proto::golem::worker::WorkerStatusFilter {
                        comparator: comparator.into(),
                        value: value.into(),
                    },
                )
            }
            WorkerFilter::CreatedAt(WorkerCreatedAtFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::CreatedAt(
                    golem_api_grpc::proto::golem::worker::WorkerCreatedAtFilter {
                        value: Some(value.into()),
                        comparator: comparator.into(),
                    },
                )
            }
            WorkerFilter::Not(WorkerNotFilter { filter }) => {
                let f: golem_api_grpc::proto::golem::worker::WorkerFilter = (*filter).into();
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Not(Box::new(
                    golem_api_grpc::proto::golem::worker::WorkerNotFilter {
                        filter: Some(Box::new(f)),
                    },
                ))
            }
            WorkerFilter::And(filter) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::And(
                    golem_api_grpc::proto::golem::worker::WorkerAndFilter {
                        filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                    },
                )
            }
            WorkerFilter::Or(filter) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Or(
                    golem_api_grpc::proto::golem::worker::WorkerOrFilter {
                        filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                    },
                )
            }
        };

        golem_api_grpc::proto::golem::worker::WorkerFilter {
            filter: Some(filter),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Enum)]
pub enum StringFilterComparator {
    Equal,
    NotEqual,
    Like,
    NotLike,
}

impl StringFilterComparator {
    pub fn matches<T: Display>(&self, value1: &T, value2: &T) -> bool {
        match self {
            StringFilterComparator::Equal => value1.to_string() == value2.to_string(),
            StringFilterComparator::NotEqual => value1.to_string() != value2.to_string(),
            StringFilterComparator::Like => {
                value1.to_string().contains(value2.to_string().as_str())
            }
            StringFilterComparator::NotLike => {
                !value1.to_string().contains(value2.to_string().as_str())
            }
        }
    }
}

impl From<StringFilterComparator> for golem_api_grpc::proto::golem::common::StringFilterComparator {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringEqual
            }
            StringFilterComparator::NotEqual => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringNotEqual
            }
            StringFilterComparator::Like => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringLike
            }
            StringFilterComparator::NotLike => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringNotLike
            }
        }
    }
}

impl FromStr for StringFilterComparator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "==" | "=" | "equal" | "eq" => Ok(StringFilterComparator::Equal),
            "!=" | "notequal" | "ne" => Ok(StringFilterComparator::NotEqual),
            "like" => Ok(StringFilterComparator::Like),
            "notlike" => Ok(StringFilterComparator::NotLike),
            _ => Err(format!("Unknown String Filter Comparator: {}", s)),
        }
    }
}

impl TryFrom<i32> for StringFilterComparator {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(StringFilterComparator::Equal),
            1 => Ok(StringFilterComparator::NotEqual),
            2 => Ok(StringFilterComparator::Like),
            3 => Ok(StringFilterComparator::NotLike),
            _ => Err(format!("Unknown String Filter Comparator: {}", value)),
        }
    }
}

impl From<StringFilterComparator> for i32 {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => 0,
            StringFilterComparator::NotEqual => 1,
            StringFilterComparator::Like => 2,
            StringFilterComparator::NotLike => 3,
        }
    }
}

impl Display for StringFilterComparator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StringFilterComparator::Equal => "==",
            StringFilterComparator::NotEqual => "!=",
            StringFilterComparator::Like => "like",
            StringFilterComparator::NotLike => "notlike",
        };
        write!(f, "{}", s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, Enum)]
pub enum FilterComparator {
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
    LessEqual,
    Less,
}

impl Display for FilterComparator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            FilterComparator::Equal => "==",
            FilterComparator::NotEqual => "!=",
            FilterComparator::GreaterEqual => ">=",
            FilterComparator::Greater => ">",
            FilterComparator::LessEqual => "<=",
            FilterComparator::Less => "<",
        };
        write!(f, "{}", s)
    }
}

impl FilterComparator {
    pub fn matches<T: Ord>(&self, value1: &T, value2: &T) -> bool {
        match self {
            FilterComparator::Equal => value1 == value2,
            FilterComparator::NotEqual => value1 != value2,
            FilterComparator::Less => value1 < value2,
            FilterComparator::LessEqual => value1 <= value2,
            FilterComparator::Greater => value1 > value2,
            FilterComparator::GreaterEqual => value1 >= value2,
        }
    }
}

impl FromStr for FilterComparator {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "==" | "=" | "equal" | "eq" => Ok(FilterComparator::Equal),
            "!=" | "notequal" | "ne" => Ok(FilterComparator::NotEqual),
            ">=" | "greaterequal" | "ge" => Ok(FilterComparator::GreaterEqual),
            ">" | "greater" | "gt" => Ok(FilterComparator::Greater),
            "<=" | "lessequal" | "le" => Ok(FilterComparator::LessEqual),
            "<" | "less" | "lt" => Ok(FilterComparator::Less),
            _ => Err(format!("Unknown Filter Comparator: {}", s)),
        }
    }
}

impl From<FilterComparator> for golem_api_grpc::proto::golem::common::FilterComparator {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => {
                golem_api_grpc::proto::golem::common::FilterComparator::Equal
            }
            FilterComparator::NotEqual => {
                golem_api_grpc::proto::golem::common::FilterComparator::NotEqual
            }
            FilterComparator::Less => golem_api_grpc::proto::golem::common::FilterComparator::Less,
            FilterComparator::LessEqual => {
                golem_api_grpc::proto::golem::common::FilterComparator::LessEqual
            }
            FilterComparator::Greater => {
                golem_api_grpc::proto::golem::common::FilterComparator::Greater
            }
            FilterComparator::GreaterEqual => {
                golem_api_grpc::proto::golem::common::FilterComparator::GreaterEqual
            }
        }
    }
}

impl TryFrom<i32> for FilterComparator {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(FilterComparator::Equal),
            1 => Ok(FilterComparator::NotEqual),
            2 => Ok(FilterComparator::Less),
            3 => Ok(FilterComparator::LessEqual),
            4 => Ok(FilterComparator::Greater),
            5 => Ok(FilterComparator::GreaterEqual),
            _ => Err(format!("Unknown Filter Comparator: {}", value)),
        }
    }
}

impl From<FilterComparator> for i32 {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => 0,
            FilterComparator::NotEqual => 1,
            FilterComparator::Less => 2,
            FilterComparator::LessEqual => 3,
            FilterComparator::Greater => 4,
            FilterComparator::GreaterEqual => 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::vec;

    use bincode::{Decode, Encode};
    use serde::{Deserialize, Serialize};

    use crate::model::{
        parse_function_name, AccountId, FilterComparator, StringFilterComparator, TemplateId,
        Timestamp, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus, WorkerStatusRecord,
    };

    #[test]
    fn timestamp_conversion() {
        let ts: Timestamp = Timestamp::now_utc();

        let prost_ts: prost_types::Timestamp = ts.into();

        let ts2: Timestamp = prost_ts.into();

        assert_eq!(ts2, ts);
    }

    #[test]
    fn parse_function_name_global() {
        let parsed = parse_function_name("run-example");
        assert_eq!(parsed.interface, None);
        assert_eq!(parsed.function, "run-example");
    }

    #[test]
    fn parse_function_name_in_exported_interface_no_package() {
        let parsed = parse_function_name("interface/fn1");
        assert_eq!(parsed.interface, Some("interface".to_string()));
        assert_eq!(parsed.function, "fn1".to_string());
    }

    #[test]
    fn parse_function_name_in_exported_interface() {
        let parsed = parse_function_name("ns:name/interface/fn1");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(parsed.function, "fn1".to_string());
    }

    #[test]
    fn parse_function_name_constructor_syntax_sugar() {
        let parsed = parse_function_name("ns:name/interface/resource1/new");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(parsed.function, "[constructor]resource1".to_string());
    }

    #[test]
    fn parse_function_name_constructor() {
        let parsed = parse_function_name("ns:name/interface/[constructor]resource1");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(parsed.function, "[constructor]resource1".to_string());
    }

    #[test]
    fn parse_function_name_method_syntax_sugar() {
        let parsed = parse_function_name("ns:name/interface/resource1/do-something");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(
            parsed.function,
            "[method]resource1.do-something".to_string()
        );
    }

    #[test]
    fn parse_function_name_method() {
        let parsed = parse_function_name("ns:name/interface/[method]resource1.do-something");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(
            parsed.function,
            "[method]resource1.do-something".to_string()
        );
    }

    #[test]
    fn parse_function_name_static_method_syntax_sugar() {
        // Note: the syntax sugared version cannot distinguish between method and static - so we need to check the actual existence of
        // the function and fallback.
        let parsed = parse_function_name("ns:name/interface/resource1/do-something-static")
            .method_as_static()
            .unwrap();
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(
            parsed.function,
            "[static]resource1.do-something-static".to_string()
        );
    }

    #[test]
    fn parse_function_name_static() {
        let parsed = parse_function_name("ns:name/interface/[static]resource1.do-something-static");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(
            parsed.function,
            "[static]resource1.do-something-static".to_string()
        );
    }

    #[test]
    fn parse_function_name_drop_syntax_sugar() {
        let parsed = parse_function_name("ns:name/interface/resource1/drop");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(parsed.function, "[drop]resource1".to_string());
    }

    #[test]
    fn parse_function_name_drop() {
        let parsed = parse_function_name("ns:name/interface/[drop]resource1");
        assert_eq!(parsed.interface, Some("ns:name/interface".to_string()));
        assert_eq!(parsed.function, "[drop]resource1".to_string());
    }

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
    fn worker_filter_parse() {
        assert_eq!(
            WorkerFilter::from_str(" name =  worker-1").unwrap(),
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
        );

        assert_eq!(
            WorkerFilter::from_str("status == Running").unwrap(),
            WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running)
        );

        assert_eq!(
            WorkerFilter::from_str("version >= 10").unwrap(),
            WorkerFilter::new_version(FilterComparator::GreaterEqual, 10)
        );

        assert_eq!(
            WorkerFilter::from_str("env.tag1 == abc ").unwrap(),
            WorkerFilter::new_env(
                "tag1".to_string(),
                StringFilterComparator::Equal,
                "abc".to_string(),
            )
        );
    }

    #[test]
    fn worker_filter_combination() {
        assert_eq!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).not(),
            WorkerFilter::new_not(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                "worker-1".to_string(),
            ))
        );

        assert_eq!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).and(
                WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running)
            ),
            WorkerFilter::new_and(vec![
                WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running),
            ])
        );

        assert_eq!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
                .and(WorkerFilter::new_status(
                    FilterComparator::Equal,
                    WorkerStatus::Running
                ))
                .and(WorkerFilter::new_version(FilterComparator::Equal, 1)),
            WorkerFilter::new_and(vec![
                WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running),
                WorkerFilter::new_version(FilterComparator::Equal, 1),
            ])
        );

        assert_eq!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).or(
                WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running)
            ),
            WorkerFilter::new_or(vec![
                WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running),
            ])
        );

        assert_eq!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
                .or(WorkerFilter::new_status(
                    FilterComparator::NotEqual,
                    WorkerStatus::Running
                ))
                .or(WorkerFilter::new_version(FilterComparator::Equal, 1)),
            WorkerFilter::new_or(vec![
                WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                WorkerFilter::new_status(FilterComparator::NotEqual, WorkerStatus::Running),
                WorkerFilter::new_version(FilterComparator::Equal, 1),
            ])
        );

        assert_eq!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
                .and(WorkerFilter::new_status(
                    FilterComparator::NotEqual,
                    WorkerStatus::Running
                ))
                .or(WorkerFilter::new_version(FilterComparator::Equal, 1)),
            WorkerFilter::new_or(vec![
                WorkerFilter::new_and(vec![
                    WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                    WorkerFilter::new_status(FilterComparator::NotEqual, WorkerStatus::Running),
                ]),
                WorkerFilter::new_version(FilterComparator::Equal, 1),
            ])
        );

        assert_eq!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
                .or(WorkerFilter::new_status(
                    FilterComparator::NotEqual,
                    WorkerStatus::Running
                ))
                .and(WorkerFilter::new_version(FilterComparator::Equal, 1)),
            WorkerFilter::new_and(vec![
                WorkerFilter::new_or(vec![
                    WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                    WorkerFilter::new_status(FilterComparator::NotEqual, WorkerStatus::Running),
                ]),
                WorkerFilter::new_version(FilterComparator::Equal, 1),
            ])
        );
    }

    #[test]
    fn worker_filter_matches() {
        let template_id = TemplateId::new_v4();
        let worker_metadata = WorkerMetadata {
            worker_id: WorkerId {
                worker_name: "worker-1".to_string(),
                template_id,
            },
            args: vec![],
            env: vec![
                ("env1".to_string(), "value1".to_string()),
                ("env2".to_string(), "value2".to_string()),
            ],
            account_id: AccountId {
                value: "account-1".to_string(),
            },
            created_at: Timestamp::now_utc(),
            last_known_status: WorkerStatusRecord {
                component_version: 1,
                ..WorkerStatusRecord::default()
            },
        };

        assert!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
                .and(WorkerFilter::new_status(
                    FilterComparator::Equal,
                    WorkerStatus::Idle
                ))
                .matches(&worker_metadata)
        );

        assert!(WorkerFilter::new_env(
            "env1".to_string(),
            StringFilterComparator::Equal,
            "value1".to_string(),
        )
        .and(WorkerFilter::new_status(
            FilterComparator::Equal,
            WorkerStatus::Idle
        ))
        .matches(&worker_metadata));

        assert!(WorkerFilter::new_env(
            "env1".to_string(),
            StringFilterComparator::Equal,
            "value2".to_string(),
        )
        .not()
        .and(
            WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Running).or(
                WorkerFilter::new_status(FilterComparator::Equal, WorkerStatus::Idle)
            )
        )
        .matches(&worker_metadata));

        assert!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
                .and(WorkerFilter::new_version(FilterComparator::Equal, 1))
                .matches(&worker_metadata)
        );

        assert!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-2".to_string())
                .or(WorkerFilter::new_version(FilterComparator::Equal, 1))
                .matches(&worker_metadata)
        );

        assert!(WorkerFilter::new_version(FilterComparator::GreaterEqual, 1)
            .and(WorkerFilter::new_version(FilterComparator::Less, 2))
            .or(WorkerFilter::new_name(
                StringFilterComparator::Equal,
                "worker-2".to_string(),
            ))
            .matches(&worker_metadata));
    }
}
