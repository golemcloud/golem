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

pub mod account;
pub mod agent;
pub mod api_domain;
pub mod application;
pub mod auth;
pub mod base64;
pub mod certificate;
pub mod component;
pub mod component_constraint;
pub mod component_metadata;
pub mod deployment;
pub mod diff;
pub mod domain_registration;
pub mod environment;
pub mod environment_plugin_grant;
pub mod environment_share;
pub mod error;
pub mod exports;
pub mod http_api_definition;
pub mod http_api_deployment;
pub mod invocation_context;
pub mod login;
pub mod lucene;
pub mod oplog;
pub mod plan;
pub mod plugin_registration;
pub mod poem;
pub mod protobuf;
pub mod regions;
pub mod reports;
pub mod security_scheme;
pub mod trim_date;
pub mod worker;

pub use crate::base_model::*;

use self::component::ComponentId;
use self::component::{ComponentFilePermissions, ComponentRevision, PluginPriority};
use self::environment::EnvironmentId;
use crate::model::account::AccountId;
use crate::model::invocation_context::InvocationContextStack;
use crate::model::oplog::{TimestampedUpdateDescription, WorkerResourceId};
use crate::model::regions::DeletedRegions;
use crate::{declare_structs, SafeDisplay};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use golem_wasm::analysis::analysed_type::{field, record, u32, u64};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{FromValue, IntoValue, Value};
use golem_wasm_derive::{FromValue, IntoValue};
use http::Uri;
use poem_openapi::{Object, Union};
use rand::prelude::IteratorRandom;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Display, Formatter, Write};
use std::ops::Add;
use std::str::FromStr;
use std::time::Duration;
use url::Url;
use uuid::{uuid, Uuid};

#[derive(Debug, Clone, Serialize, Deserialize, poem_openapi::Object)]
pub struct Page<
    T: poem_openapi::types::Type + poem_openapi::types::ParseFromJSON + poem_openapi::types::ToJSON,
> {
    pub values: Vec<T>,
}

pub trait PoemTypeRequirements:
    poem_openapi::types::Type + poem_openapi::types::ParseFromJSON + poem_openapi::types::ToJSON
{
}

impl<
        T: poem_openapi::types::Type
            + poem_openapi::types::ParseFromJSON
            + poem_openapi::types::ToJSON,
    > PoemTypeRequirements for T
{
}

pub trait PoemMultipartTypeRequirements: poem_openapi::types::ParseFromMultipartField {}

impl<T: poem_openapi::types::ParseFromMultipartField> PoemMultipartTypeRequirements for T {}

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

    pub fn rounded(self) -> Self {
        Self::from(self.to_millis())
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
            let timestamp = <i64 as Deserialize>::deserialize(deserializer)?;
            Ok(Timestamp(
                iso8601_timestamp::Timestamp::UNIX_EPOCH
                    .add(Duration::from_millis(timestamp as u64)),
            ))
        }
    }
}

impl BinarySerializer for Timestamp {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        BinarySerializer::serialize(
            &(self
                .0
                .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH)
                .whole_milliseconds() as u64),
            context,
        )
    }
}

impl BinaryDeserializer for Timestamp {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let timestamp: u64 = BinaryDeserializer::deserialize(context)?;
        Ok(Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH.add(Duration::from_millis(timestamp)),
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

impl FromValue for Timestamp {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(fields) if fields.len() == 2 => {
                let mut iter = fields.into_iter();
                let seconds = u64::from_value(iter.next().unwrap())?;
                let nanos = u32::from_value(iter.next().unwrap())?;
                Ok(Self(
                    iso8601_timestamp::Timestamp::UNIX_EPOCH
                        .add(Duration::from_secs(seconds))
                        .add(Duration::from_nanos(nanos as u64)),
                ))
            }
            other => Err(format!(
                "Expected a record with two fields for Timestamp, got {other:?}"
            )),
        }
    }
}

declare_structs! {
    pub struct VersionInfo {
        pub version: String,
    }
}

/// Associates a worker-id with its owner project
#[derive(Clone, Debug, Eq, PartialEq, Hash, BinaryCodec)]
#[desert(evolution())]
pub struct OwnedWorkerId {
    pub environment_id: EnvironmentId,
    pub worker_id: WorkerId,
}

impl OwnedWorkerId {
    pub fn new(environment_id: &EnvironmentId, worker_id: &WorkerId) -> Self {
        Self {
            environment_id: *environment_id,
            worker_id: worker_id.clone(),
        }
    }

    pub fn worker_id(&self) -> WorkerId {
        self.worker_id.clone()
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    pub fn component_id(&self) -> ComponentId {
        self.worker_id.component_id
    }

    pub fn worker_name(&self) -> String {
        self.worker_id.worker_name.clone()
    }
}

impl Display for OwnedWorkerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.environment_id(), self.worker_id)
    }
}

impl AsRef<WorkerId> for OwnedWorkerId {
    fn as_ref(&self) -> &WorkerId {
        &self.worker_id
    }
}

/// Actions that can be scheduled to be executed at a given point in time
#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum ScheduledAction {
    /// Completes a given promise
    CompletePromise {
        account_id: AccountId,
        environment_id: EnvironmentId,
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
                environment_id,
                promise_id,
                ..
            } => OwnedWorkerId::new(environment_id, &promise_id.worker_id),
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

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
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

impl Display for RoutingTable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Number of shards: {}", self.number_of_shards.value)?;
        writeln!(f, "Pods used: {:?}", self.all())?;
        Ok(())
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

#[derive(Clone, Debug, BinaryCodec, Eq, Hash, PartialEq, IntoValue, FromValue)]
#[desert(transparent)]
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
        Serialize::serialize(&self.value, serializer)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<IdempotencyKey, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = <String as Deserialize>::deserialize(deserializer)?;
        Ok(IdempotencyKey { value })
    }
}

impl Display for IdempotencyKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl From<&str> for IdempotencyKey {
    fn from(s: &str) -> Self {
        IdempotencyKey {
            value: s.to_string(),
        }
    }
}

impl From<String> for IdempotencyKey {
    fn from(s: String) -> Self {
        IdempotencyKey { value: s }
    }
}

impl FromStr for IdempotencyKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(IdempotencyKey {
            value: s.to_string(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub env: Vec<(String, String)>,
    pub environment_id: EnvironmentId,
    pub created_by: AccountId,
    pub wasi_config_vars: BTreeMap<String, String>,
    pub created_at: Timestamp,
    pub parent: Option<WorkerId>,
    pub last_known_status: WorkerStatusRecord,
    pub original_phantom_id: Option<Uuid>,
}

impl WorkerMetadata {
    pub fn default(
        worker_id: WorkerId,
        created_by: AccountId,
        environment_id: EnvironmentId,
    ) -> WorkerMetadata {
        WorkerMetadata {
            worker_id,
            env: vec![],
            environment_id,
            created_by,
            wasi_config_vars: BTreeMap::new(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: WorkerStatusRecord::default(),
            original_phantom_id: None,
        }
    }

    pub fn owned_worker_id(&self) -> OwnedWorkerId {
        OwnedWorkerId::new(&self.environment_id, &self.worker_id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec, Object)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerResourceDescription {
    pub created_at: Timestamp,
    pub resource_owner: String,
    pub resource_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
pub struct RetryConfig {
    pub max_attempts: u32,
    #[serde(with = "humantime_serde")]
    pub min_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    pub multiplier: f64,
    pub max_jitter_factor: Option<f64>,
}

impl SafeDisplay for RetryConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "max attempts: {}", self.max_attempts);
        let _ = writeln!(&mut result, "min delay: {:?}", self.min_delay);
        let _ = writeln!(&mut result, "max delay: {:?}", self.max_delay);
        let _ = writeln!(&mut result, "multiplier: {}", self.multiplier);
        if let Some(max_jitter_factor) = &self.max_jitter_factor {
            let _ = writeln!(&mut result, "max jitter factor: {max_jitter_factor:?}");
        }

        result
    }
}

/// Contains status information about a worker according to a given oplog index.
///
/// This status is just cached information, all fields must be computable by the oplog alone.
/// By having an associated oplog_idx, the cached information can be used together with the
/// tail of the oplog to determine the actual status of the worker.
#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
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
    pub component_revision: ComponentRevision,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub owned_resources: HashMap<WorkerResourceId, WorkerResourceDescription>,
    pub oplog_idx: OplogIndex,
    pub active_plugins: HashSet<PluginPriority>,
    pub deleted_regions: DeletedRegions,
    /// The component version at the starting point of the replay. Will be the version of the Create oplog entry
    /// if only automatic updates were used or the version of the latest snapshot-based update
    pub component_revision_for_replay: ComponentRevision,
    /// The number of encountered error entries grouped by their 'retry_from' index, calculated from
    /// the last invocation boundary.
    pub current_retry_count: HashMap<OplogIndex, u32>,
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
            component_revision: ComponentRevision(0),
            component_size: 0,
            total_linear_memory_size: 0,
            owned_resources: HashMap::new(),
            oplog_idx: OplogIndex::default(),
            active_plugins: HashSet::new(),
            deleted_regions: DeletedRegions::new(),
            component_revision_for_replay: ComponentRevision(0),
            current_retry_count: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct FailedUpdateRecord {
    pub timestamp: Timestamp,
    pub target_revision: ComponentRevision,
    pub details: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct SuccessfulUpdateRecord {
    pub timestamp: Timestamp,
    pub target_revision: ComponentRevision,
}

/// Represents last known status of a worker
///
/// This is always recorded together with the current oplog index, and it can only be used
/// as a source of truth if there are no newer oplog entries since the record.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
    poem_openapi::Enum,
)]
#[desert(evolution())]
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

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum WorkerInvocation {
    ManualUpdate {
        target_revision: ComponentRevision,
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

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct TimestampedWorkerInvocation {
    pub timestamp: Timestamp,
    pub invocation: WorkerInvocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[desert(evolution())]
pub struct WorkerVersionFilter {
    pub comparator: FilterComparator,
    pub value: ComponentRevision,
}

impl WorkerVersionFilter {
    pub fn new(comparator: FilterComparator, value: ComponentRevision) -> Self {
        Self { comparator, value }
    }
}

impl Display for WorkerVersionFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "version {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[desert(evolution())]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[desert(evolution())]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, Union)]
#[desert(evolution())]
#[oai(discriminator_name = "type", one_of = true)]
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
                let revision: ComponentRevision = metadata.last_known_status.component_revision;
                comparator.matches(&revision, &value)
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

    pub fn new_version(comparator: FilterComparator, value: ComponentRevision) -> Self {
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

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, poem_openapi::Enum,
)]
#[desert(evolution())]
pub enum StringFilterComparator {
    Equal,
    NotEqual,
    Like,
    NotLike,
    StartsWith,
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
            StringFilterComparator::StartsWith => {
                value1.to_string().starts_with(value2.to_string().as_str())
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
            "startswith" => Ok(StringFilterComparator::StartsWith),
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
            4 => Ok(StringFilterComparator::StartsWith),
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
            StringFilterComparator::StartsWith => 4,
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
            StringFilterComparator::StartsWith => "startswith",
        };
        write!(f, "{s}")
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec, poem_openapi::Enum,
)]
#[desert(evolution())]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BinaryCodec, Default, Object)]
#[desert(evolution())]
#[oai(rename_all = "camelCase")]
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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, BinaryCodec, Serialize, Deserialize)]
#[desert(evolution())]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

impl Display for LogLevel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
            LogLevel::Critical => "critical",
        };
        write!(f, "{}", s)
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, BinaryCodec)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntypedJsonBody(pub serde_json::Value);

impl poem_openapi::types::Type for UntypedJsonBody {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "UntypedJsonBody".into()
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        poem_openapi::registry::MetaSchemaRef::Reference(Self::name().into_owned())
    }

    fn register(registry: &mut poem_openapi::registry::Registry) {
        registry.create_schema::<Self, _>(Self::name().into_owned(), |_| {
            let mut schema = poem_openapi::registry::MetaSchema::new("object");
            schema.description = Some("A json body without a static schema");
            schema
        });
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

impl poem_openapi::types::ToJSON for UntypedJsonBody {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(self.0.clone())
    }
}

impl poem_openapi::types::ParseFromJSON for UntypedJsonBody {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        match value {
            Some(json) => Ok(Self(json)),
            _ => Err(poem_openapi::types::ParseError::<UntypedJsonBody>::custom(
                "Received empty value for UntypedJsonBody",
            )),
        }
    }
}

impl From<WorkerId> for golem_wasm::AgentId {
    fn from(worker_id: WorkerId) -> Self {
        golem_wasm::AgentId {
            component_id: worker_id.component_id.into(),
            agent_id: worker_id.worker_name,
        }
    }
}

impl From<golem_wasm::AgentId> for WorkerId {
    fn from(host: golem_wasm::AgentId) -> Self {
        Self {
            component_id: host.component_id.into(),
            worker_name: host.agent_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, IntoValue, FromValue, BinaryCodec)]
pub enum ForkResult {
    /// The original worker that called `fork`
    Original,
    /// The new worker
    Forked,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct RdbmsPoolKey {
    pub address: Url,
}

impl RdbmsPoolKey {
    pub fn new(address: Url) -> Self {
        Self { address }
    }

    pub fn from(address: &str) -> Result<Self, String> {
        let url = Url::parse(address).map_err(|e| e.to_string())?;
        Ok(Self::new(url))
    }

    pub fn masked_address(&self) -> String {
        let mut output: String = self.address.scheme().to_string();
        output.push_str("://");

        let username = self.address.username();
        output.push_str(username);

        let password = self.address.password();
        if password.is_some() {
            output.push_str(":*****");
        }

        if let Some(h) = self.address.host_str() {
            if !username.is_empty() || password.is_some() {
                output.push('@');
            }

            output.push_str(h);

            if let Some(p) = self.address.port() {
                output.push(':');
                output.push_str(p.to_string().as_str());
            }
        }

        output.push_str(self.address.path());

        let query_pairs = self.address.query_pairs();

        if query_pairs.count() > 0 {
            output.push('?');
        }
        for (index, (key, value)) in query_pairs.enumerate() {
            let key = &*key;
            output.push_str(key);
            output.push('=');

            if key == "password" || key == "secret" {
                output.push_str("*****");
            } else {
                output.push_str(&value);
            }
            if index < query_pairs.count() - 1 {
                output.push('&');
            }
        }

        output
    }
}

impl Display for RdbmsPoolKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.masked_address())
    }
}
