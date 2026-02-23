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
pub mod application;
pub mod auth;
pub mod base64;
pub mod certificate;
pub mod component;
pub mod component_metadata;
pub mod deployment;
pub mod diff;
pub mod domain_registration;
pub mod environment;
pub mod environment_plugin_grant;
pub mod environment_share;
pub mod error;
pub mod exports;
pub mod http_api_deployment;
pub mod invocation_context;
pub mod login;
pub mod lucene;
pub mod oplog;
pub mod optional_field_update;
#[cfg(feature = "full")]
pub mod parsed_function_name;
pub mod plan;
pub mod plugin_registration;
pub mod poem;
pub mod protobuf;
pub mod regions;
pub mod reports;
pub mod security_scheme;
#[cfg(test)]
mod tests;
pub mod trim_date;
pub mod worker;

pub use crate::base_model::*;

use self::component::ComponentId;
use self::component::{ComponentFilePermissions, ComponentRevision, PluginPriority};
use self::environment::EnvironmentId;
use crate::base_model::agent::AgentId;
use crate::model::account::AccountId;
use crate::model::agent::{AgentTypeResolver, UntypedDataValue, UntypedElementValue};
use crate::model::invocation_context::InvocationContextStack;
use crate::model::oplog::{
    OplogEntry, RawSnapshotData, TimestampedUpdateDescription, WorkerResourceId,
};
use crate::model::regions::DeletedRegions;
use crate::{SafeDisplay, grpc_uri};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use golem_wasm::Value;
use golem_wasm_derive::{FromValue, IntoValue};
use http::Uri;
use rand::prelude::IteratorRandom;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Display, Formatter, Write};
use std::ops::Add;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

impl WorkerId {
    const WORKER_ID_MAX_LENGTH: usize = 512;

    pub fn from_agent_id(
        component_id: ComponentId,
        agent_id: &AgentId,
    ) -> Result<WorkerId, String> {
        let agent_id = agent_id.to_string();
        if agent_id.len() > Self::WORKER_ID_MAX_LENGTH {
            return Err(format!(
                "Agent id is too long: {}, max length: {}, agent id: {}",
                agent_id.len(),
                Self::WORKER_ID_MAX_LENGTH,
                agent_id,
            ));
        }
        Ok(Self {
            component_id,
            worker_name: agent_id,
        })
    }

    pub fn from_agent_id_literal<S: AsRef<str>>(
        component_id: ComponentId,
        agent_id: S,
        resolver: impl AgentTypeResolver,
    ) -> Result<WorkerId, String> {
        Self::from_agent_id(component_id, &AgentId::parse(agent_id, resolver)?)
    }

    pub fn from_component_metadata_and_worker_id<S: AsRef<str>>(
        component_id: ComponentId,
        component_metadata: &component_metadata::ComponentMetadata,
        id: S,
    ) -> Result<WorkerId, String> {
        if component_metadata.is_agent() {
            Self::from_agent_id_literal(component_id, id, component_metadata)
        } else {
            let id = id.as_ref();
            if id.len() > Self::WORKER_ID_MAX_LENGTH {
                return Err(format!(
                    "Legacy worker id is too long: {}, max length: {}, worker id: {}",
                    id.len(),
                    Self::WORKER_ID_MAX_LENGTH,
                    id,
                ));
            }
            if id.contains('/') {
                return Err(format!(
                    "Legacy worker id cannot contain '/', worker id: {}",
                    id,
                ));
            }

            Ok(WorkerId {
                component_id,
                worker_name: id.to_string(),
            })
        }
    }

    pub fn from_worker_name_string<S: AsRef<str>>(
        component_id: ComponentId,
        id: S,
    ) -> Result<WorkerId, String> {
        let id = id.as_ref();

        match AgentId::normalize_text(id) {
            Ok(normalized) => {
                if normalized.len() > Self::WORKER_ID_MAX_LENGTH {
                    return Err(format!(
                        "Agent id is too long: {}, max length: {}, agent id: {}",
                        normalized.len(),
                        Self::WORKER_ID_MAX_LENGTH,
                        normalized,
                    ));
                }
                Ok(WorkerId {
                    component_id,
                    worker_name: normalized,
                })
            }
            Err(_) => {
                if id.len() > Self::WORKER_ID_MAX_LENGTH {
                    return Err(format!(
                        "Legacy worker id is too long: {}, max length: {}, worker id: {}",
                        id.len(),
                        Self::WORKER_ID_MAX_LENGTH,
                        id,
                    ));
                }
                if id.contains('/') {
                    return Err(format!(
                        "Legacy worker id cannot contain '/', worker id: {}",
                        id,
                    ));
                }
                Ok(WorkerId {
                    component_id,
                    worker_name: id.to_string(),
                })
            }
        }
    }
}

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
    T: poem_openapi::types::Type + poem_openapi::types::ParseFromJSON + poem_openapi::types::ToJSON,
> PoemTypeRequirements for T
{
}

pub trait PoemMultipartTypeRequirements: poem_openapi::types::ParseFromMultipartField {}

impl<T: poem_openapi::types::ParseFromMultipartField> PoemMultipartTypeRequirements for T {}

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

/// Associates a worker-id with its owner project
#[derive(Clone, Debug, Eq, PartialEq, Hash, BinaryCodec)]
#[desert(evolution())]
pub struct OwnedWorkerId {
    pub environment_id: EnvironmentId,
    pub worker_id: WorkerId,
}

impl OwnedWorkerId {
    pub fn new(environment_id: EnvironmentId, worker_id: &WorkerId) -> Self {
        Self {
            environment_id,
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
            } => OwnedWorkerId::new(*environment_id, &promise_id.worker_id),
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
    pub fn uri(&self, use_tls: bool) -> Uri {
        grpc_uri(&self.host, self.port, use_tls)
    }
}

impl Display for Pod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
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

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
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
        OwnedWorkerId::new(self.environment_id, &self.worker_id)
    }
}

impl WorkerFilter {
    pub fn matches(&self, metadata: &WorkerMetadata) -> bool {
        match self.clone() {
            WorkerFilter::Name(WorkerNameFilter { comparator, value }) => {
                comparator.matches(&metadata.worker_id.worker_name, &value)
            }
            WorkerFilter::Revision(WorkerRevisionFilter { comparator, value }) => {
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
    pub pending_invocations: Vec<TimestampedAgentInvocation>,
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
    pub last_snapshot_index: Option<OplogIndex>,
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
            component_revision: ComponentRevision::INITIAL,
            component_size: 0,
            total_linear_memory_size: 0,
            owned_resources: HashMap::new(),
            oplog_idx: OplogIndex::default(),
            active_plugins: HashSet::new(),
            deleted_regions: DeletedRegions::new(),
            component_revision_for_replay: ComponentRevision::INITIAL,
            current_retry_count: HashMap::new(),
            last_snapshot_index: None,
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

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocation {
    ManualUpdate {
        target_revision: ComponentRevision,
    },
    AgentInitialization {
        idempotency_key: IdempotencyKey,
        input: UntypedDataValue,
        invocation_context: InvocationContextStack,
    },
    AgentMethod {
        idempotency_key: IdempotencyKey,
        method_name: String,
        input: UntypedDataValue,
        invocation_context: InvocationContextStack,
    },
    LoadSnapshot {
        idempotency_key: IdempotencyKey,
        snapshot: RawSnapshotData, // TODO
    },
    SaveSnapshot {
        idempotency_key: IdempotencyKey,
        // TODO
    },
    ProcessOplogEntries {
        idempotency_key: IdempotencyKey,
        account_id: AccountId,
        config: Vec<(String, String)>,
        metadata: WorkerMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    },
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocationPayload {
    ManualUpdate {
        target_revision: ComponentRevision,
    },
    AgentInitialization {
        input: UntypedDataValue,
    },
    AgentMethod {
        method_name: String,
        input: UntypedDataValue,
    },
    LoadSnapshot {
        snapshot: RawSnapshotData,
    },
    SaveSnapshot,
    ProcessOplogEntries {
        account_id: AccountId,
        config: Vec<(String, String)>,
        metadata: WorkerMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    },
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocationResult {
    AgentInitialization { output: UntypedDataValue },
    AgentMethod { output: UntypedDataValue },
    ManualUpdate,
    LoadSnapshot { error: Option<String> },
    SaveSnapshot { snapshot: RawSnapshotData },
    ProcessOplogEntries { error: Option<String> },
}

impl AgentInvocationResult {
    /// Extracts the raw `Option<Value>` from the result, for compatibility with
    /// code paths that still work with raw wasm values.
    pub fn into_raw_output(self) -> Option<Value> {
        match self {
            AgentInvocationResult::AgentInitialization { output }
            | AgentInvocationResult::AgentMethod { output } => match output {
                UntypedDataValue::Tuple(elements) => elements.into_iter().find_map(|e| match e {
                    UntypedElementValue::ComponentModel(v) => Some(v),
                    _ => None,
                }),
                _ => None,
            },
            AgentInvocationResult::ManualUpdate => None,
            AgentInvocationResult::SaveSnapshot { snapshot } => Some(Value::Record(vec![
                Value::List(snapshot.data.into_iter().map(Value::U8).collect()),
                Value::String(snapshot.mime_type),
            ])),
            AgentInvocationResult::LoadSnapshot { error } => match error {
                Some(err) => Some(Value::Result(Err(Some(Box::new(Value::String(err)))))),
                None => Some(Value::Result(Ok(None))),
            },
            AgentInvocationResult::ProcessOplogEntries { error } => match error {
                Some(err) => Some(Value::Result(Err(Some(Box::new(Value::String(err)))))),
                None => Some(Value::Result(Ok(None))),
            },
        }
    }
}

impl AgentInvocation {
    pub fn from_parts(
        idempotency_key: IdempotencyKey,
        payload: AgentInvocationPayload,
        invocation_context: InvocationContextStack,
    ) -> Self {
        match payload {
            AgentInvocationPayload::ManualUpdate { target_revision } => {
                Self::ManualUpdate { target_revision }
            }
            AgentInvocationPayload::AgentInitialization { input } => Self::AgentInitialization {
                idempotency_key,
                input,
                invocation_context,
            },
            AgentInvocationPayload::AgentMethod { method_name, input } => Self::AgentMethod {
                idempotency_key,
                method_name,
                input,
                invocation_context,
            },
            AgentInvocationPayload::LoadSnapshot { snapshot } => Self::LoadSnapshot {
                idempotency_key,
                snapshot,
            },
            AgentInvocationPayload::SaveSnapshot => Self::SaveSnapshot { idempotency_key },
            AgentInvocationPayload::ProcessOplogEntries {
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
            } => Self::ProcessOplogEntries {
                idempotency_key,
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
            },
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        IdempotencyKey,
        AgentInvocationPayload,
        InvocationContextStack,
    ) {
        match self {
            Self::ManualUpdate { target_revision } => (
                IdempotencyKey::fresh(),
                AgentInvocationPayload::ManualUpdate { target_revision },
                InvocationContextStack::fresh(),
            ),
            Self::AgentInitialization {
                idempotency_key,
                input,
                invocation_context,
            } => (
                idempotency_key,
                AgentInvocationPayload::AgentInitialization { input },
                invocation_context,
            ),
            Self::AgentMethod {
                idempotency_key,
                method_name,
                input,
                invocation_context,
            } => (
                idempotency_key,
                AgentInvocationPayload::AgentMethod { method_name, input },
                invocation_context,
            ),
            Self::LoadSnapshot {
                idempotency_key,
                snapshot,
            } => (
                idempotency_key,
                AgentInvocationPayload::LoadSnapshot { snapshot },
                InvocationContextStack::fresh(),
            ),
            Self::SaveSnapshot { idempotency_key } => (
                idempotency_key,
                AgentInvocationPayload::SaveSnapshot,
                InvocationContextStack::fresh(),
            ),
            Self::ProcessOplogEntries {
                idempotency_key,
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
            } => (
                idempotency_key,
                AgentInvocationPayload::ProcessOplogEntries {
                    account_id,
                    config,
                    metadata,
                    first_entry_index,
                    entries,
                },
                InvocationContextStack::fresh(),
            ),
        }
    }

    pub fn has_idempotency_key(&self, key: &IdempotencyKey) -> bool {
        match self {
            Self::AgentMethod {
                idempotency_key, ..
            } => idempotency_key == key,
            Self::AgentInitialization {
                idempotency_key, ..
            } => idempotency_key == key,
            _ => false,
        }
    }

    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        match self {
            Self::AgentMethod {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::AgentInitialization {
                idempotency_key, ..
            } => Some(idempotency_key),
            _ => None,
        }
    }

    pub fn invocation_context(&self) -> InvocationContextStack {
        match self {
            Self::AgentInitialization {
                invocation_context, ..
            } => invocation_context.clone(),
            Self::AgentMethod {
                invocation_context, ..
            } => invocation_context.clone(),
            _ => InvocationContextStack::fresh(),
        }
    }

    pub fn function_name(&self) -> String {
        match self {
            Self::ManualUpdate { .. } => String::new(),
            Self::AgentInitialization { .. } => "golem:agent/guest.{initialize}".to_string(),
            Self::AgentMethod { method_name, .. } => method_name.clone(),
            Self::LoadSnapshot { .. } => "golem:api/load-snapshot.{load}".to_string(),
            Self::SaveSnapshot { .. } => "golem:api/save-snapshot.{save}".to_string(),
            Self::ProcessOplogEntries { .. } => "golem:api/oplog-processor.{process}".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct TimestampedAgentInvocation {
    pub timestamp: Timestamp,
    pub invocation: AgentInvocation,
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

impl From<PromiseId> for golem_wasm::PromiseId {
    fn from(promise_id: PromiseId) -> Self {
        golem_wasm::PromiseId {
            agent_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx.into(),
        }
    }
}

impl From<golem_wasm::PromiseId> for PromiseId {
    fn from(host: golem_wasm::PromiseId) -> Self {
        Self {
            worker_id: host.agent_id.into(),
            oplog_idx: OplogIndex::from_u64(host.oplog_idx),
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
