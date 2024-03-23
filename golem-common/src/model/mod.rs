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

use crate::config::RetryConfig;
use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use derive_more::FromStr;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ParseFromParameter, ParseResult, ToJSON};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

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
    pub oplog_idx: u64,
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

/// Contains status information about a worker according to a given oplog index.
/// This status is just cached information, all fields must be computable by the oplog alone.
/// By having an associated oplog_idx, the cached information can be used together with the
/// tail of the oplog to determine the actual status of the worker.
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct WorkerStatusRecord {
    pub status: WorkerStatus,
    pub deleted_regions: DeletedRegions,
    pub overridden_retry_config: Option<RetryConfig>,
    pub oplog_idx: u64,
}

impl Default for WorkerStatusRecord {
    fn default() -> Self {
        WorkerStatusRecord {
            status: WorkerStatus::Idle,
            deleted_regions: DeletedRegions::new(),
            overridden_retry_config: None,
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
    /// The last invocation failed and a retry was scheduled
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

#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub enum WorkerFilter {
    Empty,
    Name {
        comparator: FilterStringComparator,
        value: String,
    },
    Status {
        value: WorkerStatus,
    },
    Version {
        comparator: FilterComparator,
        value: i32,
    },
    // CreatedAt {
    //     comparator: FilterComparator,
    //     value: String,
    // },
    Env {
        name: String,
        comparator: FilterStringComparator,
        value: String,
    },
    And(Vec<WorkerFilter>),
    Or(Vec<WorkerFilter>),
    Not(Box<WorkerFilter>),
}

impl WorkerFilter {
    pub fn and(&self, filters: Vec<WorkerFilter>) -> Self {
        match self {
            WorkerFilter::Empty => Self::new_and(filters),
            _ => {
                let new_filters = [vec![self.clone()], filters].concat();

                Self::new_and(new_filters)
            }
        }
    }

    pub fn or(&self, filters: Vec<WorkerFilter>) -> Self {
        match self {
            WorkerFilter::Empty => Self::new_or(filters),
            _ => {
                let new_filters = [vec![self.clone()], filters].concat();
                Self::new_or(new_filters)
            }
        }
    }

    pub fn not(&self) -> Self {
        match self {
            WorkerFilter::Empty => self.clone(),
            _ => Self::new_not(self.clone()),
        }
    }

    pub fn eval(&self, metadata: &WorkerMetadata) -> bool {
        match self.clone() {
            WorkerFilter::Empty => true,
            WorkerFilter::Name { comparator, value } => {
                match comparator {
                    FilterStringComparator::Equal => {
                        metadata.worker_id.worker_id.worker_name == value
                    }
                    FilterStringComparator::Like => metadata
                        .worker_id
                        .worker_id
                        .worker_name
                        .contains(value.as_str()), // FIXME
                }
            }
            WorkerFilter::Version { comparator, value } => {
                let version = metadata.worker_id.template_version;
                match comparator {
                    FilterComparator::Equal => version == value,
                    FilterComparator::NotEqual => version != value,
                    FilterComparator::Less => version < value,
                    FilterComparator::LessEqual => version <= value,
                    FilterComparator::Greater => version > value,
                    FilterComparator::GreaterEqual => version >= value,
                }
            }
            WorkerFilter::Env {
                name,
                comparator,
                value,
            } => {
                let mut result = false;
                for env_value in metadata.env.clone() {
                    if env_value.0 == name {
                        result = match comparator {
                            FilterStringComparator::Equal => env_value.1 == value,
                            FilterStringComparator::Like => env_value.1.contains(value.as_str()), // FIXME
                        };

                        if result == false {
                            break;
                        }
                    }
                }
                result
            }
            WorkerFilter::Not(filter) => !filter.eval(metadata),
            WorkerFilter::Status { value } => metadata.last_known_status.status == value,
            WorkerFilter::And(filters) => {
                let mut result = true;
                for filter in filters {
                    result = filter.eval(metadata);
                    if result == false {
                        break;
                    }
                }
                result
            }
            WorkerFilter::Or(filters) => {
                let mut result = true;
                if !filters.is_empty() {
                    result = false;
                    for filter in filters {
                        result = filter.eval(metadata);
                        if result == true {
                            break;
                        }
                    }
                }
                result
            }
        }
    }

    pub fn new_and(filters: Vec<WorkerFilter>) -> Self {
        WorkerFilter::And(filters)
    }

    pub fn new_or(filters: Vec<WorkerFilter>) -> Self {
        WorkerFilter::Or(filters)
    }

    pub fn new_not(filter: WorkerFilter) -> Self {
        WorkerFilter::Not(Box::new(filter))
    }

    pub fn new_name(comparator: FilterStringComparator, value: String) -> Self {
        WorkerFilter::Name { comparator, value }
    }

    pub fn new_env(name: String, comparator: FilterStringComparator, value: String) -> Self {
        WorkerFilter::Env {
            name,
            comparator,
            value,
        }
    }

    pub fn new_version(comparator: FilterComparator, value: i32) -> Self {
        WorkerFilter::Version { comparator, value }
    }

    pub fn new_status(value: WorkerStatus) -> Self {
        WorkerFilter::Status { value }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub enum FilterStringComparator {
    Equal,
    Like,
}

#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub enum FilterComparator {
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
    LessEqual,
    Less,
}

#[cfg(test)]
mod tests {
    use bincode::{Decode, Encode};
    use serde::{Deserialize, Serialize};

    use crate::model::{parse_function_name, AccountId};

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
}
