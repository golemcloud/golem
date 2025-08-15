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

pub mod agent;
pub mod auth;
pub mod base64;
pub mod component;
pub mod component_constraint;
#[cfg(feature = "tokio")]
pub mod component_metadata;
pub mod error;
pub mod exports;
pub mod invocation_context;
pub mod lucene;
pub mod oplog;
pub mod plugin;
#[cfg(feature = "poem")]
mod poem;
pub mod project;
#[cfg(feature = "protobuf")]
pub mod protobuf;
pub mod public_oplog;
pub mod regions;
pub mod trim_date;
pub mod worker;

pub use crate::base_model::*;
use crate::model::agent::DataValue;
use crate::model::invocation_context::InvocationContextStack;
use crate::model::oplog::{TimestampedUpdateDescription, WorkerResourceId};
use crate::model::regions::DeletedRegions;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use golem_wasm_ast::analysis::analysed_type::{field, list, record, str, tuple, u32, u64};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{IntoValue, Value};
use golem_wasm_rpc_derive::IntoValue;
use http::Uri;
use rand::prelude::IteratorRandom;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Display, Formatter};
use std::ops::Add;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use typed_path::Utf8UnixPathBuf;
use uuid::{uuid, Uuid};

#[cfg(feature = "poem")]
pub trait PoemTypeRequirements:
    poem_openapi::types::Type + poem_openapi::types::ParseFromJSON + poem_openapi::types::ToJSON
{
}

#[cfg(not(feature = "poem"))]
pub trait PoemTypeRequirements {}

#[cfg(feature = "poem")]
impl<
        T: poem_openapi::types::Type
            + poem_openapi::types::ParseFromJSON
            + poem_openapi::types::ToJSON,
    > PoemTypeRequirements for T
{
}

#[cfg(not(feature = "poem"))]
impl<T> PoemTypeRequirements for T {}

#[cfg(feature = "poem")]
pub trait PoemMultipartTypeRequirements: poem_openapi::types::ParseFromMultipartField {}

#[cfg(not(feature = "poem"))]
pub trait PoemMultipartTypeRequirements {}

#[cfg(feature = "poem")]
impl<T: poem_openapi::types::ParseFromMultipartField> PoemMultipartTypeRequirements for T {}

#[cfg(not(feature = "poem"))]
impl<T> PoemMultipartTypeRequirements for T {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Timestamp(iso8601_timestamp::Timestamp);

impl Timestamp {
    pub fn now_utc() -> Timestamp {
        Timestamp(iso8601_timestamp::Timestamp::now_utc())
    }

    pub fn to_millis(&self) -> u64 {
        self.0
            .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH)
            .whole_milliseconds() as u64
    }
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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
        D: Deserializer<'de>,
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

impl<Context> bincode::Decode<Context> for Timestamp {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let timestamp: i64 = bincode::Decode::decode(decoder)?;
        Ok(Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(timestamp as u64)),
        ))
    }
}

impl<'de, Context> bincode::BorrowDecode<'de, Context> for Timestamp {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let timestamp: i64 = bincode::BorrowDecode::borrow_decode(decoder)?;
        Ok(Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(timestamp as u64)),
        ))
    }
}

impl From<u64> for Timestamp {
    fn from(value: u64) -> Self {
        Timestamp(iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(value)))
    }
}

impl IntoValue for Timestamp {
    fn into_value(self) -> Value {
        let d = self
            .0
            .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH);
        Value::Record(vec![
            Value::U64(d.whole_seconds() as u64),
            Value::U32(d.subsec_nanoseconds() as u32),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![field("seconds", u64()), field("nanoseconds", u32())])
    }
}

/// Associates a worker-id with its owner project
#[derive(Clone, Debug, Eq, PartialEq, Hash, Encode, Decode)]
pub struct OwnedWorkerId {
    pub project_id: ProjectId,
    pub worker_id: WorkerId,
}

impl OwnedWorkerId {
    pub fn new(project_id: &ProjectId, worker_id: &WorkerId) -> Self {
        Self {
            project_id: project_id.clone(),
            worker_id: worker_id.clone(),
        }
    }

    pub fn worker_id(&self) -> WorkerId {
        self.worker_id.clone()
    }

    pub fn project_id(&self) -> ProjectId {
        self.project_id.clone()
    }

    pub fn component_id(&self) -> ComponentId {
        self.worker_id.component_id.clone()
    }

    pub fn worker_name(&self) -> String {
        self.worker_id.worker_name.clone()
    }
}

impl Display for OwnedWorkerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.project_id, self.worker_id)
    }
}

impl AsRef<WorkerId> for OwnedWorkerId {
    fn as_ref(&self) -> &WorkerId {
        &self.worker_id
    }
}

/// Actions that can be scheduled to be executed at a given point in time
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum ScheduledAction {
    /// Completes a given promise
    CompletePromise {
        account_id: AccountId,
        project_id: ProjectId,
        promise_id: PromiseId,
    },
    /// Archives all entries from the first non-empty layer of an oplog to the next layer,
    /// if the last oplog index did not change. If there are more layers below, schedules
    /// a next action to archive the next layer.
    ArchiveOplog {
        account_id: AccountId,
        owned_worker_id: OwnedWorkerId,
        last_oplog_index: OplogIndex,
        next_after: Duration,
    },
    /// Invoke the given action on the worker. The invocation will only
    /// be persisted in the oplog when it's actually getting scheduled.
    Invoke {
        account_id: AccountId,
        owned_worker_id: OwnedWorkerId,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    },
}

impl ScheduledAction {
    pub fn owned_worker_id(&self) -> OwnedWorkerId {
        match self {
            ScheduledAction::CompletePromise {
                project_id,
                promise_id,
                ..
            } => OwnedWorkerId::new(project_id, &promise_id.worker_id),
            ScheduledAction::ArchiveOplog {
                owned_worker_id, ..
            } => owned_worker_id.clone(),
            ScheduledAction::Invoke {
                owned_worker_id, ..
            } => owned_worker_id.clone(),
        }
    }
}

impl Display for ScheduledAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduledAction::CompletePromise { promise_id, .. } => {
                write!(f, "complete[{promise_id}]")
            }
            ScheduledAction::ArchiveOplog {
                owned_worker_id, ..
            } => {
                write!(f, "archive[{owned_worker_id}]")
            }
            ScheduledAction::Invoke {
                owned_worker_id, ..
            } => write!(f, "invoke[{owned_worker_id}]"),
        }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct ScheduleId {
    pub timestamp: i64,
    pub action: ScheduledAction,
}

impl Display for ScheduleId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.action, self.timestamp)
    }
}

#[derive(Clone)]
pub struct NumberOfShards {
    pub value: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Pod {
    host: String,
    port: u16,
}

impl Pod {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build URI")
    }
}

#[derive(Clone)]
pub struct RoutingTable {
    pub number_of_shards: NumberOfShards,
    shard_assignments: HashMap<ShardId, Pod>,
}

impl RoutingTable {
    pub fn lookup(&self, worker_id: &WorkerId) -> Option<&Pod> {
        self.shard_assignments.get(&ShardId::from_worker_id(
            &worker_id.clone(),
            self.number_of_shards.value,
        ))
    }

    pub fn random(&self) -> Option<&Pod> {
        self.shard_assignments.values().choose(&mut rand::rng())
    }

    pub fn first(&self) -> Option<&Pod> {
        self.shard_assignments.values().next()
    }

    pub fn all(&self) -> HashSet<&Pod> {
        self.shard_assignments.values().collect()
    }
}

#[allow(dead_code)]
pub struct RoutingTableEntry {
    shard_id: ShardId,
    pod: Pod,
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

#[derive(Clone, Debug, Encode, Decode, Eq, Hash, PartialEq, IntoValue)]
#[wit_transparent]
pub struct IdempotencyKey {
    pub value: String,
}

impl IdempotencyKey {
    const ROOT_NS: Uuid = uuid!("9C19B15A-C83D-46F7-9BC3-EAD7923733F4");

    pub fn new(value: String) -> Self {
        Self { value }
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self {
            value: value.to_string(),
        }
    }

    pub fn fresh() -> Self {
        Self::from_uuid(Uuid::new_v4())
    }

    /// Generates a deterministic new idempotency key using a base idempotency key and an oplog index.
    ///
    /// The base idempotency key determines the "namespace" of the generated key UUIDv5. If
    /// the base idempotency key is already an UUID, it is directly used as the namespace of the v5 algorithm,
    /// while the name part is derived from the given oplog index.
    ///
    /// If the base idempotency key is not an UUID (as it can be an arbitrary user-provided string), then first
    /// we generate a UUIDv5 in the ROOT_NS namespace and use that as unique namespace for generating
    /// the new idempotency key.
    pub fn derived(base: &IdempotencyKey, oplog_index: OplogIndex) -> Self {
        let namespace = if let Ok(base_uuid) = Uuid::parse_str(&base.value) {
            base_uuid
        } else {
            Uuid::new_v5(&Self::ROOT_NS, base.value.as_bytes())
        };
        let name = format!("oplog-index-{oplog_index}");
        Self::from_uuid(Uuid::new_v5(&namespace, name.as_bytes()))
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<IdempotencyKey, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(IdempotencyKey { value })
    }
}

impl Display for IdempotencyKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[derive(Clone, Debug)]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub project_id: ProjectId,
    pub created_by: AccountId,
    pub wasi_config_vars: BTreeMap<String, String>,
    pub created_at: Timestamp,
    pub parent: Option<WorkerId>,
    pub last_known_status: WorkerStatusRecord,
}

impl WorkerMetadata {
    pub fn default(
        worker_id: WorkerId,
        created_by: AccountId,
        project_id: ProjectId,
    ) -> WorkerMetadata {
        WorkerMetadata {
            worker_id,
            args: vec![],
            env: vec![],
            project_id,
            created_by,
            wasi_config_vars: BTreeMap::new(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: WorkerStatusRecord::default(),
        }
    }

    pub fn owned_worker_id(&self) -> OwnedWorkerId {
        OwnedWorkerId::new(&self.project_id, &self.worker_id)
    }
}

impl IntoValue for WorkerMetadata {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.worker_id.into_value(),
            self.args.into_value(),
            self.env.into_value(),
            self.wasi_config_vars.into_value(),
            self.last_known_status.status.into_value(),
            self.last_known_status.component_version.into_value(),
            0u64.into_value(), // retry count could be computed from the worker status record here but we don't support it yet
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("worker-id", WorkerId::get_type()),
            field("args", list(str())),
            field("env", list(tuple(vec![str(), str()]))),
            field("wasi-config-vars", HashMap::<String, String>::get_type()),
            field("status", WorkerStatus::get_type()),
            field("component-version", u64()),
            field("retry-count", u64()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ExportedResourceInstanceKey {
    pub resource_id: WorkerResourceId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ExportedResourceInstanceDescription {
    pub created_at: Timestamp,
    pub resource_owner: String,
    pub resource_name: String,
    pub resource_params: Option<Vec<String>>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    Encode,
    Decode,
    IntoValue,
)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct AgentInstanceKey {
    pub agent_type: String,
    pub agent_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct AgentInstanceDescription {
    pub created_at: Timestamp,
    pub agent_parameters: DataValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum WorkerResourceKey {
    /// A living resource instance that has been returned through invocation
    /// and can be referenced to from outside the worker
    ExportedResourceInstanceKey(ExportedResourceInstanceKey),
    /// An agent instance
    AgentInstanceKey(AgentInstanceKey),
}

impl Display for WorkerResourceKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerResourceKey::ExportedResourceInstanceKey(key) => {
                write!(f, "resource({})", key.resource_id)
            }
            WorkerResourceKey::AgentInstanceKey(key) => {
                write!(f, "agent({}, {})", key.agent_type, key.agent_id)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum WorkerResourceDescription {
    /// A living resource instance that has been returned through invocation
    /// and can be referenced to from outside the worker
    ExportedResourceInstance(ExportedResourceInstanceDescription),
    /// An agent instance
    AgentInstance(AgentInstanceDescription),
}

impl WorkerResourceDescription {
    pub fn with_timestamp(&self, new_timestamp: Timestamp) -> Self {
        match self {
            WorkerResourceDescription::ExportedResourceInstance(desc) => {
                WorkerResourceDescription::ExportedResourceInstance(
                    ExportedResourceInstanceDescription {
                        created_at: new_timestamp,
                        resource_owner: desc.resource_owner.clone(),
                        resource_name: desc.resource_name.clone(),
                        resource_params: desc.resource_params.clone(),
                    },
                )
            }
            WorkerResourceDescription::AgentInstance(desc) => {
                WorkerResourceDescription::AgentInstance(AgentInstanceDescription {
                    created_at: new_timestamp,
                    agent_parameters: desc.agent_parameters.clone(),
                })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct RetryConfig {
    pub max_attempts: u32,
    #[serde(with = "humantime_serde")]
    pub min_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    pub multiplier: f64,
    pub max_jitter_factor: Option<f64>,
}

/// Contains status information about a worker according to a given oplog index.
///
/// This status is just cached information, all fields must be computable by the oplog alone.
/// By having an associated oplog_idx, the cached information can be used together with the
/// tail of the oplog to determine the actual status of the worker.
#[derive(Clone, Debug, PartialEq, Encode)]
pub struct WorkerStatusRecord {
    pub status: WorkerStatus,
    pub skipped_regions: DeletedRegions,
    pub overridden_retry_config: Option<RetryConfig>,
    pub pending_invocations: Vec<TimestampedWorkerInvocation>,
    pub pending_updates: VecDeque<TimestampedUpdateDescription>,
    pub failed_updates: Vec<FailedUpdateRecord>,
    pub successful_updates: Vec<SuccessfulUpdateRecord>,
    pub invocation_results: HashMap<IdempotencyKey, OplogIndex>,
    pub current_idempotency_key: Option<IdempotencyKey>,
    pub component_version: ComponentVersion,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub owned_resources: HashMap<WorkerResourceKey, WorkerResourceDescription>,
    pub oplog_idx: OplogIndex,
    pub active_plugins: HashSet<PluginInstallationId>,
    pub deleted_regions: DeletedRegions,
    /// The component version at the starting point of the replay. Will be the version of the Create oplog entry
    /// if only automatic updates were used or the version of the latest snapshot based update
    pub component_version_for_replay: ComponentVersion,
}

impl<Context> bincode::Decode<Context> for WorkerStatusRecord {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        Ok(Self {
            status: Decode::decode(decoder)?,
            skipped_regions: Decode::decode(decoder)?,
            overridden_retry_config: Decode::decode(decoder)?,
            pending_invocations: Decode::decode(decoder)?,
            pending_updates: Decode::decode(decoder)?,
            failed_updates: Decode::decode(decoder)?,
            successful_updates: Decode::decode(decoder)?,
            invocation_results: Decode::decode(decoder)?,
            current_idempotency_key: Decode::decode(decoder)?,
            component_version: Decode::decode(decoder)?,
            component_size: Decode::decode(decoder)?,
            total_linear_memory_size: Decode::decode(decoder)?,
            owned_resources: Decode::decode(decoder)?,
            oplog_idx: Decode::decode(decoder)?,
            active_plugins: Decode::decode(decoder)?,
            deleted_regions: Decode::decode(decoder)?,
            component_version_for_replay: Decode::decode(decoder)?,
        })
    }
}
impl<'de, Context> BorrowDecode<'de, Context> for WorkerStatusRecord {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        Ok(Self {
            status: BorrowDecode::borrow_decode(decoder)?,
            skipped_regions: BorrowDecode::borrow_decode(decoder)?,
            overridden_retry_config: BorrowDecode::borrow_decode(decoder)?,
            pending_invocations: BorrowDecode::borrow_decode(decoder)?,
            pending_updates: BorrowDecode::borrow_decode(decoder)?,
            failed_updates: BorrowDecode::borrow_decode(decoder)?,
            successful_updates: BorrowDecode::borrow_decode(decoder)?,
            invocation_results: BorrowDecode::borrow_decode(decoder)?,
            current_idempotency_key: BorrowDecode::borrow_decode(decoder)?,
            component_version: BorrowDecode::borrow_decode(decoder)?,
            component_size: BorrowDecode::borrow_decode(decoder)?,
            total_linear_memory_size: BorrowDecode::borrow_decode(decoder)?,
            owned_resources: BorrowDecode::borrow_decode(decoder)?,
            oplog_idx: BorrowDecode::borrow_decode(decoder)?,
            active_plugins: BorrowDecode::borrow_decode(decoder)?,
            deleted_regions: BorrowDecode::borrow_decode(decoder)?,
            component_version_for_replay: BorrowDecode::borrow_decode(decoder)?,
        })
    }
}

impl Default for WorkerStatusRecord {
    fn default() -> Self {
        WorkerStatusRecord {
            status: WorkerStatus::Idle,
            skipped_regions: DeletedRegions::new(),
            overridden_retry_config: None,
            pending_invocations: Vec::new(),
            pending_updates: VecDeque::new(),
            failed_updates: Vec::new(),
            successful_updates: Vec::new(),
            invocation_results: HashMap::new(),
            current_idempotency_key: None,
            component_version: 0,
            component_size: 0,
            total_linear_memory_size: 0,
            owned_resources: HashMap::new(),
            oplog_idx: OplogIndex::default(),
            active_plugins: HashSet::new(),
            deleted_regions: DeletedRegions::new(),
            component_version_for_replay: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct FailedUpdateRecord {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub details: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct SuccessfulUpdateRecord {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
}

/// Represents last known status of a worker
///
/// This is always recorded together with the current oplog index, and it can only be used
/// as a source of truth if there are no newer oplog entries since the record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
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
            _ => Err(format!("Unknown worker status: {s}")),
        }
    }
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerStatus::Running => write!(f, "Running"),
            WorkerStatus::Idle => write!(f, "Idle"),
            WorkerStatus::Suspended => write!(f, "Suspended"),
            WorkerStatus::Interrupted => write!(f, "Interrupted"),
            WorkerStatus::Retrying => write!(f, "Retrying"),
            WorkerStatus::Failed => write!(f, "Failed"),
            WorkerStatus::Exited => write!(f, "Exited"),
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
            _ => Err(format!("Unknown worker status: {value}")),
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

/// Internal representation of `WorkerInvocation` to support backward compatibility
/// in its binary format.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
enum SerializedWorkerInvocation {
    ExportedFunctionV1 {
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
    },
    ManualUpdate {
        target_version: ComponentVersion,
    },
    ExportedFunction {
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    },
}

impl From<WorkerInvocation> for SerializedWorkerInvocation {
    fn from(value: WorkerInvocation) -> Self {
        match value {
            WorkerInvocation::ManualUpdate { target_version } => {
                Self::ManualUpdate { target_version }
            }
            WorkerInvocation::ExportedFunction {
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context,
            } => Self::ExportedFunction {
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context,
            },
        }
    }
}

impl From<SerializedWorkerInvocation> for WorkerInvocation {
    fn from(value: SerializedWorkerInvocation) -> Self {
        match value {
            SerializedWorkerInvocation::ExportedFunctionV1 {
                idempotency_key,
                full_function_name,
                function_input,
            } => Self::ExportedFunction {
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context: InvocationContextStack::fresh(),
            },
            SerializedWorkerInvocation::ManualUpdate { target_version } => {
                Self::ManualUpdate { target_version }
            }
            SerializedWorkerInvocation::ExportedFunction {
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context,
            } => Self::ExportedFunction {
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkerInvocation {
    ManualUpdate {
        target_version: ComponentVersion,
    },
    ExportedFunction {
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
        invocation_context: InvocationContextStack,
    },
}

impl WorkerInvocation {
    pub fn is_idempotency_key(&self, key: &IdempotencyKey) -> bool {
        match self {
            Self::ExportedFunction {
                idempotency_key, ..
            } => idempotency_key == key,
            _ => false,
        }
    }

    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        match self {
            Self::ExportedFunction {
                idempotency_key, ..
            } => Some(idempotency_key),
            _ => None,
        }
    }

    pub fn invocation_context(&self) -> InvocationContextStack {
        match self {
            Self::ExportedFunction {
                invocation_context, ..
            } => invocation_context.clone(),
            _ => InvocationContextStack::fresh(),
        }
    }
}

impl Encode for WorkerInvocation {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        let serialized: SerializedWorkerInvocation = self.clone().into();
        serialized.encode(encoder)
    }
}

impl<Context> Decode<Context> for WorkerInvocation {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let serialized: SerializedWorkerInvocation = Decode::decode(decoder)?;
        Ok(serialized.into())
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for WorkerInvocation {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let serialized: SerializedWorkerInvocation = BorrowDecode::borrow_decode(decoder)?;
        Ok(serialized.into())
    }
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
    derive_more::FromStr,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
    Encode,
    Decode,
    IntoValue,
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

impl Display for AccountId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WorkerWasiConfigVarsFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl WorkerWasiConfigVarsFilter {
    pub fn new(name: String, comparator: StringFilterComparator, value: String) -> Self {
        Self {
            name,
            comparator,
            value,
        }
    }
}

impl Display for WorkerWasiConfigVarsFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "wasi_config_vars.{} {} {}",
            self.name, self.comparator, self.value
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum WorkerFilter {
    Name(WorkerNameFilter),
    Status(WorkerStatusFilter),
    Version(WorkerVersionFilter),
    CreatedAt(WorkerCreatedAtFilter),
    Env(WorkerEnvFilter),
    And(WorkerAndFilter),
    Or(WorkerOrFilter),
    Not(WorkerNotFilter),
    WasiConfigVars(WorkerWasiConfigVarsFilter),
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
            WorkerFilter::WasiConfigVars(WorkerWasiConfigVarsFilter {
                name,
                comparator,
                value,
            }) => {
                let env_value = metadata.wasi_config_vars.get(&name);
                env_value
                    .map(|ev| comparator.matches(ev, &value))
                    .unwrap_or(false)
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

    pub fn new_wasi_config_vars(
        name: String,
        comparator: StringFilterComparator,
        value: String,
    ) -> Self {
        WorkerFilter::WasiConfigVars(WorkerWasiConfigVarsFilter::new(name, comparator, value))
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
                write!(f, "{filter}")
            }
            WorkerFilter::Version(filter) => {
                write!(f, "{filter}")
            }
            WorkerFilter::Status(filter) => {
                write!(f, "{filter}")
            }
            WorkerFilter::CreatedAt(filter) => {
                write!(f, "{filter}")
            }
            WorkerFilter::Env(filter) => {
                write!(f, "{filter}")
            }
            WorkerFilter::WasiConfigVars(filter) => {
                write!(f, "{filter}")
            }
            WorkerFilter::Not(filter) => {
                write!(f, "{filter}")
            }
            WorkerFilter::And(filter) => {
                write!(f, "{filter}")
            }
            WorkerFilter::Or(filter) => {
                write!(f, "{filter}")
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
                        .map_err(|e| format!("Invalid filter value: {e}"))?,
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
                _ => Err(format!("Invalid filter: {s}")),
            }
        } else {
            Err(format!("Invalid filter: {s}"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
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

impl FromStr for StringFilterComparator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "==" | "=" | "equal" | "eq" => Ok(StringFilterComparator::Equal),
            "!=" | "notequal" | "ne" => Ok(StringFilterComparator::NotEqual),
            "like" => Ok(StringFilterComparator::Like),
            "notlike" => Ok(StringFilterComparator::NotLike),
            _ => Err(format!("Unknown String Filter Comparator: {s}")),
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
            _ => Err(format!("Unknown String Filter Comparator: {value}")),
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
        write!(f, "{s}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
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
        write!(f, "{s}")
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
            _ => Err(format!("Unknown Filter Comparator: {s}")),
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
            _ => Err(format!("Unknown Filter Comparator: {value}")),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, Default)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ScanCursor {
    pub cursor: u64,
    pub layer: usize,
}

impl ScanCursor {
    pub fn is_active_layer_finished(&self) -> bool {
        self.cursor == 0
    }

    pub fn is_finished(&self) -> bool {
        self.cursor == 0 && self.layer == 0
    }

    pub fn into_option(self) -> Option<Self> {
        if self.is_finished() {
            None
        } else {
            Some(self)
        }
    }
}

impl Display for ScanCursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.layer, self.cursor)
    }
}

impl FromStr for ScanCursor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts = s.split('/').collect::<Vec<&str>>();
        if parts.len() == 2 {
            Ok(ScanCursor {
                layer: parts[0]
                    .parse()
                    .map_err(|e| format!("Invalid layer part: {e}"))?,
                cursor: parts[1]
                    .parse()
                    .map_err(|e| format!("Invalid cursor part: {e}"))?,
            })
        } else {
            Err("Invalid cursor, must have 'layer/cursor' format".to_string())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkerEvent {
    StdOut {
        timestamp: Timestamp,
        bytes: Vec<u8>,
    },
    StdErr {
        timestamp: Timestamp,
        bytes: Vec<u8>,
    },
    Log {
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
    },
    InvocationStart {
        timestamp: Timestamp,
        function: String,
        idempotency_key: IdempotencyKey,
    },
    InvocationFinished {
        timestamp: Timestamp,
        function: String,
        idempotency_key: IdempotencyKey,
    },
    /// The client fell behind and the point it left of is no longer in our buffer.
    /// {number_of_skipped_messages} is the number of messages between the client left of and the point it is now at.
    ClientLagged { number_of_missed_messages: u64 },
}

impl Display for WorkerEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerEvent::StdOut { bytes, .. } => {
                write!(
                    f,
                    "<stdout> {}",
                    String::from_utf8(bytes.clone()).unwrap_or_default()
                )
            }
            WorkerEvent::StdErr { bytes, .. } => {
                write!(
                    f,
                    "<stderr> {}",
                    String::from_utf8(bytes.clone()).unwrap_or_default()
                )
            }
            WorkerEvent::Log {
                level,
                context,
                message,
                ..
            } => {
                write!(f, "<log> {level:?} {context} {message}")
            }
            WorkerEvent::InvocationStart {
                function,
                idempotency_key,
                ..
            } => {
                write!(f, "<invocation-start> {function} {idempotency_key}")
            }
            WorkerEvent::InvocationFinished {
                function,
                idempotency_key,
                ..
            } => {
                write!(f, "<invocation-finished> {function} {idempotency_key}")
            }
            WorkerEvent::ClientLagged {
                number_of_missed_messages,
            } => {
                write!(f, "<client-lagged> {number_of_missed_messages}")
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Encode, Decode, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
#[repr(i32)]
pub enum ComponentType {
    Durable = 0,
    Ephemeral = 1,
}

impl TryFrom<i32> for ComponentType {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(ComponentType::Durable),
            1 => Ok(ComponentType::Ephemeral),
            _ => Err(format!("Unknown Component Type: {value}")),
        }
    }
}

impl Display for ComponentType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ComponentType::Durable => "Durable",
            ComponentType::Ephemeral => "Ephemeral",
        };
        write!(f, "{s}")
    }
}

impl FromStr for ComponentType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Durable" => Ok(ComponentType::Durable),
            "Ephemeral" => Ok(ComponentType::Ephemeral),
            _ => Err(format!("Unknown Component Type: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct Empty {}

/// Key that can be used to identify a component file.
/// All files with the same content will have the same key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
pub struct InitialComponentFileKey(pub String);

impl Display for InitialComponentFileKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Path inside a component filesystem. Must be
/// - absolute (start with '/')
/// - not contain ".." components
/// - not contain "." components
/// - use '/' as a separator
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ComponentFilePath(Utf8UnixPathBuf);

impl ComponentFilePath {
    pub fn from_abs_str(s: &str) -> Result<Self, String> {
        let buf: Utf8UnixPathBuf = s.into();
        if !buf.is_absolute() {
            return Err("Path must be absolute".to_string());
        }

        Ok(ComponentFilePath(buf.normalize()))
    }

    pub fn from_rel_str(s: &str) -> Result<Self, String> {
        Self::from_abs_str(&format!("/{s}"))
    }

    pub fn from_either_str(s: &str) -> Result<Self, String> {
        if s.starts_with('/') {
            Self::from_abs_str(s)
        } else {
            Self::from_rel_str(s)
        }
    }

    pub fn as_path(&self) -> &Utf8UnixPathBuf {
        &self.0
    }

    pub fn to_rel_string(&self) -> String {
        self.0.strip_prefix("/").unwrap().to_string()
    }

    pub fn extend(&mut self, path: &str) -> Result<(), String> {
        self.0.push_checked(path).map_err(|e| e.to_string())
    }
}

impl Display for ComponentFilePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for ComponentFilePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        String::serialize(&self.to_string(), serializer)
    }
}

impl<'de> Deserialize<'de> for ComponentFilePath {
    fn deserialize<D>(deserializer: D) -> Result<ComponentFilePath, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = String::deserialize(deserializer)?;
        Self::from_abs_str(&str).map_err(de::Error::custom)
    }
}

impl TryFrom<&str> for ComponentFilePath {
    type Error = String;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_either_str(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "poem", oai(rename_all = "kebab-case"))]
pub enum ComponentFilePermissions {
    ReadOnly,
    ReadWrite,
}

impl ComponentFilePermissions {
    pub fn as_compact_str(&self) -> &'static str {
        match self {
            ComponentFilePermissions::ReadOnly => "ro",
            ComponentFilePermissions::ReadWrite => "rw",
        }
    }
    pub fn from_compact_str(s: &str) -> Result<Self, String> {
        match s {
            "ro" => Ok(ComponentFilePermissions::ReadOnly),
            "rw" => Ok(ComponentFilePermissions::ReadWrite),
            _ => Err(format!("Unknown permissions: {s}")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct InitialComponentFile {
    pub key: InitialComponentFileKey,
    pub path: ComponentFilePath,
    pub permissions: ComponentFilePermissions,
}

impl InitialComponentFile {
    pub fn is_read_only(&self) -> bool {
        self.permissions == ComponentFilePermissions::ReadOnly
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentFilePathWithPermissions {
    pub path: ComponentFilePath,
    pub permissions: ComponentFilePermissions,
}

impl ComponentFilePathWithPermissions {
    pub fn extend_path(&mut self, path: &str) -> Result<(), String> {
        self.path.extend(path)
    }
}

impl Display for ComponentFilePathWithPermissions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(self).unwrap())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentFilePathWithPermissionsList {
    pub values: Vec<ComponentFilePathWithPermissions>,
}

impl Display for ComponentFilePathWithPermissionsList {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(self).unwrap())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GetFileSystemNodeResult {
    Ok(Vec<ComponentFileSystemNode>),
    File(ComponentFileSystemNode),
    NotFound,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComponentFileSystemNodeDetails {
    File {
        permissions: ComponentFilePermissions,
        size: u64,
    },
    Directory,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentFileSystemNode {
    pub name: String,
    pub last_modified: SystemTime,
    pub details: ComponentFileSystemNodeDetails,
}

// Custom Deserialize is replaced with Simple Deserialize
#[derive(Debug, Clone, PartialEq, Serialize, Encode, Decode, Default, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "poem", oai(rename_all = "kebab-case"))]
pub enum GatewayBindingType {
    #[default]
    Default,
    FileServer,
    HttpHandler,
    CorsPreflight,
    SwaggerUi,
}

impl TryFrom<String> for GatewayBindingType {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "default" => Ok(GatewayBindingType::Default),
            "file-server" => Ok(GatewayBindingType::FileServer),
            _ => Err(format!("Invalid WorkerBindingType: {value}")),
        }
    }
}

impl From<WorkerId> for golem_wasm_rpc::WorkerId {
    fn from(worker_id: WorkerId) -> Self {
        golem_wasm_rpc::WorkerId {
            component_id: worker_id.component_id.into(),
            worker_name: worker_id.worker_name,
        }
    }
}

impl From<golem_wasm_rpc::WorkerId> for WorkerId {
    fn from(host: golem_wasm_rpc::WorkerId) -> Self {
        Self {
            component_id: host.component_id.into(),
            worker_name: host.worker_name,
        }
    }
}

impl From<golem_wasm_rpc::ComponentId> for ComponentId {
    fn from(host: golem_wasm_rpc::ComponentId) -> Self {
        let high_bits = host.uuid.high_bits;
        let low_bits = host.uuid.low_bits;

        Self(Uuid::from_u64_pair(high_bits, low_bits))
    }
}

impl From<ComponentId> for golem_wasm_rpc::ComponentId {
    fn from(component_id: ComponentId) -> Self {
        let (high_bits, low_bits) = component_id.0.as_u64_pair();

        golem_wasm_rpc::ComponentId {
            uuid: golem_wasm_rpc::Uuid {
                high_bits,
                low_bits,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::str::FromStr;
    use std::time::SystemTime;
    use std::vec;
    use test_r::test;
    use tracing::info;

    use crate::model::oplog::OplogIndex;

    use crate::model::{
        AccountId, ComponentFilePath, ComponentId, FilterComparator, IdempotencyKey, ProjectId,
        ShardId, StringFilterComparator, TargetWorkerId, Timestamp, WorkerFilter, WorkerId,
        WorkerMetadata, WorkerStatus, WorkerStatusRecord,
    };
    use bincode::{Decode, Encode};

    use rand::{rng, Rng};
    use serde::{Deserialize, Serialize};

    #[test]
    fn timestamp_conversion() {
        let ts: Timestamp = Timestamp::now_utc();

        let prost_ts: prost_types::Timestamp = ts.into();

        let ts2: Timestamp = prost_ts.into();

        assert_eq!(ts2, ts);
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
                    WorkerStatus::Running,
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
                    WorkerStatus::Running,
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
                    WorkerStatus::Running,
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
                    WorkerStatus::Running,
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
        let component_id = ComponentId::new_v4();
        let worker_metadata = WorkerMetadata {
            worker_id: WorkerId {
                worker_name: "worker-1".to_string(),
                component_id,
            },
            args: vec![],
            env: vec![
                ("env1".to_string(), "value1".to_string()),
                ("env2".to_string(), "value2".to_string()),
            ],
            project_id: ProjectId::new_v4(),
            created_by: AccountId {
                value: "account-1".to_string(),
            },
            wasi_config_vars: BTreeMap::from([("var1".to_string(), "value1".to_string())]),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: WorkerStatusRecord {
                component_version: 1,
                ..WorkerStatusRecord::default()
            },
        };

        assert!(
            WorkerFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
                .and(WorkerFilter::new_status(
                    FilterComparator::Equal,
                    WorkerStatus::Idle,
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
            WorkerStatus::Idle,
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

        assert!(WorkerFilter::new_wasi_config_vars(
            "var1".to_string(),
            StringFilterComparator::Equal,
            "value1".to_string(),
        )
        .matches(&worker_metadata));

        assert!(!WorkerFilter::new_wasi_config_vars(
            "var1".to_string(),
            StringFilterComparator::Equal,
            "value2".to_string(),
        )
        .matches(&worker_metadata));
    }

    #[test]
    fn target_worker_id_force_shards() {
        let mut rng = rng();
        const SHARD_COUNT: usize = 1000;
        const EXAMPLE_COUNT: usize = 1000;
        for _ in 0..EXAMPLE_COUNT {
            let mut shard_ids = HashSet::new();
            let count = rng.random_range(0..100);
            for _ in 0..count {
                let shard_id = rng.random_range(0..SHARD_COUNT);
                shard_ids.insert(ShardId {
                    value: shard_id as i64,
                });
            }

            let component_id = ComponentId::new_v4();
            let target_worker_id = TargetWorkerId {
                component_id,
                worker_name: None,
            };

            let start = SystemTime::now();
            let worker_id = target_worker_id.into_worker_id(&shard_ids, SHARD_COUNT);
            let end = SystemTime::now();
            info!(
                "Time with {count} valid shards: {:?}",
                end.duration_since(start).unwrap()
            );

            if !shard_ids.is_empty() {
                assert!(shard_ids.contains(&ShardId::from_worker_id(&worker_id, SHARD_COUNT)));
            }
        }
    }

    #[test]
    fn derived_idempotency_key() {
        let base1 = IdempotencyKey::fresh();
        let base2 = IdempotencyKey::fresh();
        let base3 = IdempotencyKey {
            value: "base3".to_string(),
        };

        assert_ne!(base1, base2);

        let idx1 = OplogIndex::from_u64(2);
        let idx2 = OplogIndex::from_u64(11);

        let derived11a = IdempotencyKey::derived(&base1, idx1);
        let derived12a = IdempotencyKey::derived(&base1, idx2);
        let derived21a = IdempotencyKey::derived(&base2, idx1);
        let derived22a = IdempotencyKey::derived(&base2, idx2);

        let derived11b = IdempotencyKey::derived(&base1, idx1);
        let derived12b = IdempotencyKey::derived(&base1, idx2);
        let derived21b = IdempotencyKey::derived(&base2, idx1);
        let derived22b = IdempotencyKey::derived(&base2, idx2);

        let derived31 = IdempotencyKey::derived(&base3, idx1);
        let derived32 = IdempotencyKey::derived(&base3, idx2);

        assert_eq!(derived11a, derived11b);
        assert_eq!(derived12a, derived12b);
        assert_eq!(derived21a, derived21b);
        assert_eq!(derived22a, derived22b);

        assert_ne!(derived11a, derived12a);
        assert_ne!(derived11a, derived21a);
        assert_ne!(derived11a, derived22a);
        assert_ne!(derived12a, derived21a);
        assert_ne!(derived12a, derived22a);
        assert_ne!(derived21a, derived22a);

        assert_ne!(derived11a, derived31);
        assert_ne!(derived21a, derived31);
        assert_ne!(derived12a, derived32);
        assert_ne!(derived22a, derived32);
        assert_ne!(derived31, derived32);
    }

    #[test]
    fn initial_component_file_path_from_absolute() {
        let path = ComponentFilePath::from_abs_str("/a/b/c").unwrap();
        assert_eq!(path.to_string(), "/a/b/c");
    }

    #[test]
    fn initial_component_file_path_from_relative() {
        let path = ComponentFilePath::from_abs_str("a/b/c");
        assert!(path.is_err());
    }
}
