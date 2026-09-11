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

pub mod account;
pub mod account_usage;
pub mod agent;
pub mod application;
pub mod auth;
pub mod base64;
pub mod card;
pub mod certificate;
pub mod component;
pub mod component_metadata;
pub mod deployment;
pub mod diff;
pub mod domain_registration;
pub mod entity;
pub mod environment;
pub mod environment_plugin_grant;
pub mod environment_tool_grant;
pub mod error;
pub mod http_api_deployment;
pub mod invocation_context;
pub mod invocation_session_public;
pub mod login;
pub mod lucene;
pub mod mcp_deployment;
pub mod oplog;
pub mod parsed_function_name;
pub mod permission_share;
pub mod plan;
pub mod plugin_registration;
pub mod poem;
pub mod protobuf;
pub mod quota;
pub mod regions;
pub mod reports;
pub mod retry_policy;
pub mod security_scheme;
#[cfg(test)]
mod tests;
pub mod tool_release;
pub mod worker;

pub use crate::base_model::*;
pub use retry_policy::{
    FixedRng, NamedRetryPolicy, Predicate, PredicateValue, PredicateValueType, RetryContext,
    RetryEvaluationError, RetryPolicy, RetryPolicyState, RetryProperties, RetryVerdict, RngSource,
    ThreadRng,
};

use self::component::ComponentId;
use self::component::{AgentFilePermissions, ComponentRevision};
use self::environment::EnvironmentId;
use self::oplog::QueuedCardEvent;
use self::worker::{AgentConfigEntryDto, TypedAgentConfigEntry};
use crate::base_model::agent::AgentMode;
use crate::base_model::agent::Principal;
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::model::account::{AccountEmail, AccountId};
use crate::model::agent::{AgentTypeSchemaResolver, ParsedAgentId};
use crate::model::card::{CardId, ScopeCard, StoredCard};
use crate::model::invocation_context::InvocationContextStack;
use crate::model::oplog::types::AgentMetadataForGuests;
use crate::model::oplog::{AgentResourceId, OplogEntry, RawSnapshotData};
use crate::model::regions::DeletedRegions;
use crate::schema::{ResultValuePayload, SchemaValue};
use crate::{SafeDisplay, grpc_uri};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use http::Uri;
use im::{OrdMap, Vector};
use rand::prelude::IteratorRandom;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Display, Formatter, Write};
use std::net::{IpAddr, SocketAddr};
use std::ops::Add;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tonic::transport::Endpoint;
use url::Url;
use uuid::Uuid;

/// Status of an idempotency key lookup on a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationStatus {
    /// The idempotency key is not known (never seen or expired).
    Unknown,
    /// The invocation is queued or currently executing.
    Pending,
    /// The invocation has completed (successfully or with an error).
    Complete,
}

impl AgentId {
    const AGENT_ID_MAX_LENGTH: usize = 512;

    pub fn validate_length(id: &str) -> Result<(), String> {
        if id.len() > Self::AGENT_ID_MAX_LENGTH {
            Err(format!(
                "Agent id is too long: {}, max length: {}, agent id: {}",
                id.len(),
                Self::AGENT_ID_MAX_LENGTH,
                id,
            ))
        } else {
            Ok(())
        }
    }

    pub fn from_agent_id(
        component_id: ComponentId,
        agent_id: &ParsedAgentId,
    ) -> Result<AgentId, String> {
        let agent_id = agent_id.to_string();
        Self::validate_length(&agent_id)?;
        Ok(Self {
            component_id,
            agent_id,
        })
    }

    pub fn from_agent_id_literal<S: AsRef<str>>(
        component_id: ComponentId,
        agent_id: S,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<AgentId, String> {
        Self::from_agent_id(component_id, &ParsedAgentId::parse(agent_id, resolver)?)
    }

    pub fn from_component_metadata_and_agent_id<S: AsRef<str>>(
        component_id: ComponentId,
        component_metadata: &component_metadata::ComponentMetadata,
        id: S,
    ) -> Result<AgentId, String> {
        if component_metadata.is_agent() {
            Self::from_agent_id_literal(component_id, id, component_metadata)
        } else {
            let id = id.as_ref();
            if id.len() > Self::AGENT_ID_MAX_LENGTH {
                return Err(format!(
                    "Legacy worker id is too long: {}, max length: {}, worker id: {}",
                    id.len(),
                    Self::AGENT_ID_MAX_LENGTH,
                    id,
                ));
            }
            if id.contains('/') {
                return Err(format!(
                    "Legacy worker id cannot contain '/', worker id: {}",
                    id,
                ));
            }

            Ok(AgentId {
                component_id,
                agent_id: id.to_string(),
            })
        }
    }

    pub fn from_agent_name_string<S: AsRef<str>>(
        component_id: ComponentId,
        id: S,
    ) -> Result<AgentId, String> {
        let id = id.as_ref();

        match ParsedAgentId::normalize_text(id) {
            Ok(normalized) => {
                if normalized.len() > Self::AGENT_ID_MAX_LENGTH {
                    return Err(format!(
                        "Agent id is too long: {}, max length: {}, agent id: {}",
                        normalized.len(),
                        Self::AGENT_ID_MAX_LENGTH,
                        normalized,
                    ));
                }
                Ok(AgentId {
                    component_id,
                    agent_id: normalized,
                })
            }
            Err(_) => {
                if id.len() > Self::AGENT_ID_MAX_LENGTH {
                    return Err(format!(
                        "Legacy worker id is too long: {}, max length: {}, worker id: {}",
                        id.len(),
                        Self::AGENT_ID_MAX_LENGTH,
                        id,
                    ));
                }
                if id.contains('/') {
                    return Err(format!(
                        "Legacy worker id cannot contain '/', worker id: {}",
                        id,
                    ));
                }
                Ok(AgentId {
                    component_id,
                    agent_id: id.to_string(),
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

/// Associates an agent-id with its owner project
#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, BinaryCodec,
)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub struct OwnedAgentId {
    pub environment_id: EnvironmentId,
    pub agent_id: AgentId,
}

impl OwnedAgentId {
    pub fn new(environment_id: EnvironmentId, agent_id: &AgentId) -> Self {
        Self {
            environment_id,
            agent_id: agent_id.clone(),
        }
    }

    pub fn agent_id(&self) -> AgentId {
        self.agent_id.clone()
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    pub fn component_id(&self) -> ComponentId {
        self.agent_id.component_id
    }

    pub fn agent_name(&self) -> String {
        self.agent_id.agent_id.clone()
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        self
    }
}

impl Display for OwnedAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.environment_id(), self.agent_id)
    }
}

impl AsRef<AgentId> for OwnedAgentId {
    fn as_ref(&self) -> &AgentId {
        &self.agent_id
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
    #[desert(evolution(FieldAdded("agent_mode", AgentMode::Durable)))]
    ArchiveOplog {
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: OplogIndex,
        next_after: Duration,
    },
    /// Invoke the given action on the worker. The invocation will only
    /// be persisted in the oplog when it's actually getting scheduled.
    ///
    /// `target_worker_fingerprint` guards against stale invocations: when `Some(fp)`, the
    /// scheduler compares `fp` against the current worker's `fingerprint` before firing. A
    /// mismatch means the original worker was deleted and a new one was created with the same
    /// ID — the invocation is silently dropped. `None` means fire unconditionally (used for
    /// legacy entries and non-wasm-rpc invocations).
    Invoke {
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        invocation: Box<AgentInvocation>,
        target_worker_fingerprint: AgentFingerprint,
    },
    /// Resume the agent
    Resume {
        agent_created_by: AccountId,
        owned_agent_id: OwnedAgentId,
    },
    /// Invokes a fresh ephemeral agent without pre-creating it. The scheduler uses the checked
    /// creation path because delivery may be retried after an ambiguous failure.
    InvokeEphemeral {
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        invocation: Box<AgentInvocation>,
        component_revision: ComponentRevision,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
        parent: Option<AgentId>,
        creation_principal: Box<Principal>,
    },
}

impl ScheduledAction {
    pub fn owned_agent_id(&self) -> OwnedAgentId {
        match self {
            ScheduledAction::CompletePromise {
                environment_id,
                promise_id,
                ..
            } => OwnedAgentId::new(*environment_id, &promise_id.agent_id),
            ScheduledAction::ArchiveOplog { owned_agent_id, .. } => owned_agent_id.clone(),
            ScheduledAction::Invoke { owned_agent_id, .. }
            | ScheduledAction::InvokeEphemeral { owned_agent_id, .. } => owned_agent_id.clone(),
            ScheduledAction::Resume { owned_agent_id, .. } => owned_agent_id.clone(),
        }
    }
}

impl Display for ScheduledAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduledAction::CompletePromise { promise_id, .. } => {
                write!(f, "complete[{promise_id}]")
            }
            ScheduledAction::ArchiveOplog { owned_agent_id, .. } => {
                write!(f, "archive[{owned_agent_id}]")
            }
            ScheduledAction::Invoke { owned_agent_id, .. }
            | ScheduledAction::InvokeEphemeral { owned_agent_id, .. } => {
                write!(f, "invoke[{owned_agent_id}]")
            }
            ScheduledAction::Resume { owned_agent_id, .. } => write!(f, "resume[{owned_agent_id}]"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BinaryCodec)]
#[desert(evolution())]
pub struct ScheduleId {
    pub id: Uuid,
}

impl ScheduleId {
    pub fn fresh() -> Self {
        Self { id: Uuid::now_v7() }
    }

    pub fn from_idempotency_key(key: &IdempotencyKey) -> Self {
        Self {
            id: Uuid::parse_str(&key.value).expect("derived idempotency key must be a UUID"),
        }
    }
}

impl Display for ScheduleId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

#[derive(Debug, Clone)]
pub struct NumberOfShards {
    pub value: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BinaryCodec)]
#[desert(evolution())]
pub struct Pod {
    pub ip: IpAddr,
    pub port: u16,
}

impl Pod {
    pub fn uri(&self, use_tls: bool) -> Uri {
        grpc_uri(&self.ip.to_string(), self.port, use_tls)
    }

    pub fn address(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }

    pub fn endpoint(&self, use_tls: bool) -> Endpoint {
        Endpoint::from(self.uri(use_tls))
    }
}

impl Display for Pod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.ip, self.port)
    }
}

#[derive(Debug, Clone)]
pub struct RoutingTable {
    pub number_of_shards: NumberOfShards,
    shard_assignments: HashMap<ShardId, Pod>,
}

impl RoutingTable {
    pub fn lookup(&self, agent_id: &AgentId) -> Option<&Pod> {
        self.shard_assignments.get(&ShardId::from_agent_id(
            &agent_id.clone(),
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

/// Ownership generation of a single shard, granted by the shard manager.
///
/// The epoch advances only when a shard changes owner, never on a lease
/// renewal, so an executor can assert the set it believes it holds without the
/// assertion racing the manager. A newtype of this crate's own: the shard
/// manager keeps a twin in `sharding::model`, and the wire's `u64` is the only
/// bridge between them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardEpoch(pub u64);

impl Display for ShardEpoch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The revision of the shard manager's persisted state that a delivered shard
/// set was read from. Every delivery carries one - a registration, a push, a
/// renewal - and an executor applies a delivery only if its revision is at
/// least the last one it applied, so two deliveries that cross on the network
/// cannot leave the older set in place. `0` is "nothing applied yet". The
/// executor's own newtype; it never imports the shard manager's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardLeaseRevision(pub u64);

impl Display for ShardLeaseRevision {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What applying a delivered shard set did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardDeliveryOutcome {
    /// Applied. `set_changed` is whether the owned set moved, which decides
    /// whether agents on dropped shards are swept and agents on gained shards
    /// recovered.
    Applied { set_changed: bool },
    /// Older than a delivery already applied, so ignored whole.
    Stale {
        delivered: ShardLeaseRevision,
        applied: ShardLeaseRevision,
    },
}

/// The shards this executor currently holds, with the epoch each was granted
/// at, and when the lease over them lapses on this executor's own clock, if
/// it lapses at all.
#[derive(Clone, Debug, Default)]
pub struct ShardAssignment {
    pub number_of_shards: usize,
    /// Exactly the shards this executor holds. The shard manager pushes the
    /// complete set; anything absent from it has been dropped.
    pub shard_epochs: HashMap<ShardId, ShardEpoch>,
    /// When the shard lease lapses, on this executor's own monotonic clock.
    /// Only a grant moves it - the answer to this executor's own registration
    /// or renewal - and the grant is anchored where that request was sent, so
    /// the shard manager's copy of the lease is never earlier than this one.
    /// A push carries no lease. `None` means the lease never expires
    /// (single-shard mode, the debugging service, and the pre-registration
    /// placeholder).
    pub expires_at: Option<Instant>,
    /// The revision of the delivery this set came from. A delivery older than
    /// this is ignored; see [`ShardLeaseRevision`].
    pub revision: ShardLeaseRevision,
}

impl ShardAssignment {
    /// An assignment that never expires, with every shard at epoch 0. Used by
    /// the single-shard implementations and by tests, which have no shard
    /// manager to grant epochs.
    pub fn unexpiring(
        number_of_shards: usize,
        shard_ids: impl IntoIterator<Item = ShardId>,
    ) -> Self {
        Self {
            number_of_shards,
            shard_epochs: shard_ids
                .into_iter()
                .map(|shard_id| (shard_id, ShardEpoch::default()))
                .collect(),
            expires_at: None,
            revision: ShardLeaseRevision::default(),
        }
    }

    pub fn contains(&self, shard_id: &ShardId) -> bool {
        self.shard_epochs.contains_key(shard_id)
    }

    pub fn is_empty(&self) -> bool {
        self.shard_epochs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.shard_epochs.len()
    }

    pub fn shard_ids(&self) -> impl Iterator<Item = ShardId> + '_ {
        self.shard_epochs.keys().copied()
    }

    pub fn shard_id_set(&self) -> HashSet<ShardId> {
        self.shard_epochs.keys().copied().collect()
    }

    pub fn epoch_of(&self, shard_id: &ShardId) -> Option<ShardEpoch> {
        self.shard_epochs.get(shard_id).copied()
    }

    /// The claim sent on a lease renewal: exactly the set last received, in a
    /// deterministic order.
    pub fn claim(&self) -> BTreeMap<ShardId, ShardEpoch> {
        self.shard_epochs
            .iter()
            .map(|(shard_id, epoch)| (*shard_id, *epoch))
            .collect()
    }

    /// Full replace from a push: hold exactly these shards, drop everything
    /// else. One of the three doors a delivery comes through, with
    /// [`Self::adopt_grant`] and [`Self::revoke_shards`]; all three gate the
    /// set on the revision the same way in [`Self::apply`]. The lease is
    /// untouched: a push has no request of this executor's to anchor a lease
    /// to, so it carries none.
    pub fn set_shards(
        &mut self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        self.apply(Some(number_of_shards), shard_epochs, revision)
    }

    /// A grant: the answer to this executor's own registration
    /// (`number_of_shards` is `Some`) or renewal, carrying the shard manager's
    /// set for it and a lease anchored where the request was sent.
    ///
    /// The lease clock always moves, whatever becomes of the set. A grant
    /// answers a request this executor made, anchored before the request went
    /// out, so its expiry is never later than the manager's; and since a push
    /// carries no lease, refusing a grant's would cost this executor a renewal
    /// every time a rebalance push crossed a renewal reply - which is how an
    /// executor the manager keeps renewing would end up fencing itself. The
    /// set goes through [`Self::apply`]'s revision gate like every other
    /// delivery: the revision orders sets and the request time orders leases,
    /// and the two are independent. Normally the set is exactly what was
    /// claimed, because a renewal never advances an epoch; when it is not, the
    /// manager is correcting a push this executor never received, and the
    /// caller sweeps and recovers agents exactly as it would for a push.
    ///
    /// The expiry is written, never maxed against the current one: a manager
    /// whose lease length was reduced across a restart holds the shorter
    /// expiry, and grants arrive in the order they were asked for (the renewal
    /// loop awaits each one), so a later grant is always anchored later.
    pub fn adopt_grant(
        &mut self,
        number_of_shards: Option<usize>,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        self.expires_at = Some(expires_at);
        self.apply(number_of_shards, shard_epochs, revision)
    }

    /// The one place any delivery's set is applied, including
    /// [`Self::revoke_shards`], which turns its delta into the set it leaves
    /// behind and comes through here.
    ///
    /// Two deliveries can cross on the network - a renewal reply computed
    /// before a push, arriving after it - and both say "hold exactly this", so
    /// without an order the older set would win. The revision is that order: a
    /// set older than the last one applied is ignored. Equal revisions come
    /// from the same persisted state and carry the same set, so they apply
    /// harmlessly. The lease is not this function's business: only
    /// [`Self::adopt_grant`] moves it, and it does so before coming here.
    fn apply(
        &mut self,
        number_of_shards: Option<usize>,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        if revision < self.revision {
            return ShardDeliveryOutcome::Stale {
                delivered: revision,
                applied: self.revision,
            };
        }
        let set_changed = self.shard_epochs != *shard_epochs;
        if let Some(number_of_shards) = number_of_shards {
            self.number_of_shards = number_of_shards;
        }
        self.shard_epochs = shard_epochs.clone();
        self.revision = revision;
        ShardDeliveryOutcome::Applied { set_changed }
    }

    /// A revoke: `shard_ids` are dropped and everything else is kept.
    ///
    /// The one delta among the deliveries, but applied as the set it leaves behind, so it goes
    /// through the same gate as the rest: a grant read before the shards moved and arriving after
    /// this is older, and cannot put them back. Like any push it carries no lease and touches none.
    ///
    /// Adopting the revoke's revision is safe even though this is a delta, because the shard
    /// manager sends the full set at that same revision in the same pass: nothing older than it is
    /// still needed.
    pub fn revoke_shards(
        &mut self,
        shard_ids: &HashSet<ShardId>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        let mut remaining = self.shard_epochs.clone();
        remaining.retain(|shard_id, _| !shard_ids.contains(shard_id));
        self.apply(None, &remaining, revision)
    }

    /// Drops every shard, keeping `number_of_shards`, and leaves the lease
    /// **lapsed** as of `now`. Used when the shard manager no longer knows this
    /// executor's lease.
    ///
    /// The expiry is not reset to `None`, because `None` means
    /// "never expires". A cleared assignment must read as not ready, so
    /// admission keeps refusing until a re-registration installs a fresh grant.
    pub fn clear(&mut self, now: Instant) {
        self.shard_epochs.clear();
        self.expires_at = Some(now);
    }

    /// The single place the never-expires rule is spelled: a `None` expiry is
    /// always live.
    pub fn lease_is_live(&self, now: Instant) -> bool {
        match self.expires_at {
            None => true,
            Some(expires_at) => now < expires_at,
        }
    }
}

impl Display for ShardAssignment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut entries = self.shard_epochs.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(shard_id, _)| shard_id.value);
        let shard_epochs = entries
            .into_iter()
            .map(|(shard_id, epoch)| format!("{shard_id}@{epoch}"))
            .collect::<Vec<_>>()
            .join(",");
        write!(
            f,
            "{{ number_of_shards: {}, shard_epochs: {}, lease: {} }}",
            self.number_of_shards,
            shard_epochs,
            match self.expires_at {
                Some(expires_at) => match expires_at.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => format!("{}ms left", left.as_millis()),
                    _ => "lapsed".to_string(),
                },
                None => "never expires".to_string(),
            }
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentMetadata {
    pub agent_id: AgentId,
    pub env: Vec<(String, String)>,
    pub environment_id: EnvironmentId,
    pub created_by: AccountId,
    pub created_by_email: AccountEmail,
    pub config: Vec<TypedAgentConfigEntry>,
    pub created_at: Timestamp,
    pub parent: Option<AgentId>,
    pub last_known_status: AgentStatusRecord,
    pub original_phantom_id: Option<Uuid>,
    pub fingerprint: AgentFingerprint,
    pub agent_mode: AgentMode,
}

impl AgentMetadata {
    pub fn owned_agent_id(&self) -> OwnedAgentId {
        OwnedAgentId::new(self.environment_id, &self.agent_id)
    }
}

impl AgentFilter {
    pub fn matches(&self, metadata: &AgentMetadata) -> bool {
        match self.clone() {
            AgentFilter::Name(AgentNameFilter { comparator, value }) => {
                comparator.matches(&metadata.agent_id.agent_id, &value)
            }
            AgentFilter::Revision(AgentRevisionFilter { comparator, value }) => {
                let revision: ComponentRevision = metadata.last_known_status.component_revision;
                comparator.matches(&revision, &value)
            }
            AgentFilter::Env(AgentEnvFilter {
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
            AgentFilter::Config(AgentConfigVarsFilter {
                name,
                comparator,
                value,
            }) => {
                let config_value = metadata.config.iter().find_map(|entry| {
                    entry
                        .to_flat_pair()
                        .and_then(|(key, rendered_value)| (key == name).then_some(rendered_value))
                });
                config_value
                    .as_ref()
                    .map(|ev| comparator.matches(ev, &value))
                    .unwrap_or(false)
            }
            AgentFilter::CreatedAt(AgentCreatedAtFilter { comparator, value }) => {
                comparator.matches(&metadata.created_at, &value)
            }
            AgentFilter::Status(AgentStatusFilter { comparator, value }) => {
                comparator.matches(&metadata.last_known_status.status, &value)
            }
            AgentFilter::Mode(AgentModeFilter { comparator, value }) => {
                comparator.matches(&metadata.agent_mode, &value)
            }
            AgentFilter::Not(AgentNotFilter { filter }) => !filter.matches(metadata),
            AgentFilter::And(AgentAndFilter { filters }) => {
                let mut result = true;
                for filter in filters {
                    if !filter.matches(metadata) {
                        result = false;
                        break;
                    }
                }
                result
            }
            AgentFilter::Or(AgentOrFilter { filters }) => {
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

pub const DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY: usize = 1024;
// Four probes keep the false-positive rate below 2% through 100 times the default exact capacity.
pub const DEFAULT_INVOCATION_RESULT_BLOOM_BITS: usize = 1 << 20;
pub const DEFAULT_INVOCATION_RESULT_BLOOM_HASHES: u8 = 4;

/// A fixed-size, persistent Bloom filter used to prove that unseen idempotency keys are new
/// without consulting the physical invocation-result index. False positives are allowed; false
/// negatives are not.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub struct InvocationResultBloom {
    words: Vector<u64>,
    bit_count: usize,
    hash_count: u8,
}

impl InvocationResultBloom {
    pub fn new(bit_count: usize, hash_count: u8) -> Self {
        assert!(
            bit_count > 0,
            "invocation result Bloom filter must not be empty"
        );
        assert!(
            hash_count > 0,
            "invocation result Bloom filter must use a hash"
        );
        let word_count = bit_count.div_ceil(u64::BITS as usize);
        Self {
            words: std::iter::repeat_n(0, word_count).collect(),
            bit_count,
            hash_count,
        }
    }

    pub fn insert(&mut self, key: &IdempotencyKey) {
        for bit in self.bit_indexes(key) {
            let word_index = bit / u64::BITS as usize;
            let bit_index = bit % u64::BITS as usize;
            let word = self.words[word_index] | (1u64 << bit_index);
            self.words.set(word_index, word);
        }
    }

    pub fn might_contain(&self, key: &IdempotencyKey) -> bool {
        self.bit_indexes(key).all(|bit| {
            let word_index = bit / u64::BITS as usize;
            let bit_index = bit % u64::BITS as usize;
            self.words[word_index] & (1u64 << bit_index) != 0
        })
    }

    fn bit_indexes(&self, key: &IdempotencyKey) -> impl Iterator<Item = usize> + use<> {
        let digest = blake3::hash(key.value.as_bytes());
        let bytes = digest.as_bytes();
        let first = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let second = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) | 1;
        let bit_count = self.bit_count as u64;
        let hash_count = self.hash_count;
        (0..hash_count).map(move |index| {
            first
                .wrapping_add((index as u64).wrapping_mul(second))
                .wrapping_rem(bit_count) as usize
        })
    }
}

impl Default for InvocationResultBloom {
    fn default() -> Self {
        Self::new(
            DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
            DEFAULT_INVOCATION_RESULT_BLOOM_HASHES,
        )
    }
}

// A deterministic approximation of the persistent tree node, pointers, and scalar value. Dynamic
// idempotency-key bytes are accounted for separately. This only controls when the representation
// switches; neither correctness nor the eventual memory bound depends on exact allocator sizing.
const INVOCATION_RESULT_MAP_ENTRY_OVERHEAD_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
enum InvocationResultMembershipState {
    Exact {
        by_key: OrdMap<IdempotencyKey, OplogIndex>,
        key_bytes: usize,
    },
    Indexed {
        recent_by_key: OrdMap<IdempotencyKey, OplogIndex>,
        recent_by_index: OrdMap<OplogIndex, IdempotencyKey>,
        bloom: InvocationResultBloom,
    },
}

/// An exact projection of completed invocation results that switches to a bounded representation
/// once the estimated exact-map footprint exceeds the Bloom filter plus its recent exact entries.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub struct InvocationResultMembership {
    state: InvocationResultMembershipState,
    capacity: usize,
    bloom_bits: usize,
    bloom_hashes: u8,
    change_generation: u64,
    /// Number of revert entries folded into this status. It identifies the current oplog branch
    /// so physical and hydrated result entries from an earlier branch are never reused.
    revert_generation: u64,
}

impl InvocationResultMembership {
    pub fn new(capacity: usize, bloom_bits: usize, bloom_hashes: u8) -> Self {
        Self {
            state: InvocationResultMembershipState::Exact {
                by_key: OrdMap::new(),
                key_bytes: 0,
            },
            capacity,
            bloom_bits,
            bloom_hashes,
            change_generation: 0,
            revert_generation: 0,
        }
    }

    pub fn get(&self, key: &IdempotencyKey) -> Option<&OplogIndex> {
        match &self.state {
            InvocationResultMembershipState::Exact { by_key, .. } => by_key.get(key),
            InvocationResultMembershipState::Indexed { recent_by_key, .. } => {
                recent_by_key.get(key)
            }
        }
    }

    pub fn contains_key(&self, key: &IdempotencyKey) -> bool {
        self.get(key).is_some()
    }

    pub fn insert(&mut self, key: IdempotencyKey, result_index: OplogIndex) {
        self.change_generation = self.change_generation.wrapping_add(1);
        match &mut self.state {
            InvocationResultMembershipState::Exact { by_key, key_bytes } => {
                if !by_key.contains_key(&key) {
                    *key_bytes = key_bytes.saturating_add(key.value.len());
                }
                by_key.insert(key, result_index);
                self.promote_if_needed();
            }
            InvocationResultMembershipState::Indexed {
                recent_by_key,
                recent_by_index,
                bloom,
            } => {
                bloom.insert(&key);
                if let Some(previous_index) = recent_by_key.remove(&key) {
                    recent_by_index.remove(&previous_index);
                }
                recent_by_key.insert(key.clone(), result_index);
                recent_by_index.insert(result_index, key);
                Self::truncate_recent(self.capacity, recent_by_key, recent_by_index);
            }
        }
    }

    pub fn might_contain(&self, key: &IdempotencyKey) -> bool {
        match &self.state {
            InvocationResultMembershipState::Exact { by_key, .. } => by_key.contains_key(key),
            InvocationResultMembershipState::Indexed { bloom, .. } => bloom.might_contain(key),
        }
    }

    pub fn is_exact_complete(&self) -> bool {
        matches!(self.state, InvocationResultMembershipState::Exact { .. })
    }

    pub fn oldest_retained_index(&self) -> Option<OplogIndex> {
        match &self.state {
            InvocationResultMembershipState::Exact { by_key, .. } => by_key.values().copied().min(),
            InvocationResultMembershipState::Indexed {
                recent_by_index, ..
            } => recent_by_index.get_min().map(|(index, _)| *index),
        }
    }

    /// Changes whenever a result is added to or removed from the exact membership. It allows
    /// in-process admission checks to ignore unrelated oplog commits while still detecting that a
    /// result may have appeared and subsequently been evicted.
    pub fn change_generation(&self) -> u64 {
        self.change_generation
    }

    /// Returns the number of revert entries folded into this status. A change means cached result
    /// entries may belong to an obsolete oplog branch even when their indexes still exist.
    pub fn revert_generation(&self) -> u64 {
        self.revert_generation
    }

    pub fn set_revert_generation(&mut self, generation: u64) {
        self.revert_generation = generation;
    }

    pub fn iter(&self) -> impl Iterator<Item = (&IdempotencyKey, &OplogIndex)> {
        match &self.state {
            InvocationResultMembershipState::Exact { by_key, .. } => by_key.iter(),
            InvocationResultMembershipState::Indexed { recent_by_key, .. } => recent_by_key.iter(),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &IdempotencyKey> {
        match &self.state {
            InvocationResultMembershipState::Exact { by_key, .. } => by_key.keys(),
            InvocationResultMembershipState::Indexed { recent_by_key, .. } => recent_by_key.keys(),
        }
    }

    pub fn len(&self) -> usize {
        match &self.state {
            InvocationResultMembershipState::Exact { by_key, .. } => by_key.len(),
            InvocationResultMembershipState::Indexed { recent_by_key, .. } => recent_by_key.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn remove(&mut self, key: &IdempotencyKey) -> Option<OplogIndex> {
        let index = match &mut self.state {
            InvocationResultMembershipState::Exact { by_key, key_bytes } => {
                let index = by_key.remove(key)?;
                *key_bytes = key_bytes.saturating_sub(key.value.len());
                index
            }
            InvocationResultMembershipState::Indexed {
                recent_by_key,
                recent_by_index,
                ..
            } => {
                let index = recent_by_key.remove(key)?;
                recent_by_index.remove(&index);
                index
            }
        };
        self.change_generation = self.change_generation.wrapping_add(1);
        Some(index)
    }

    fn promote_if_needed(&mut self) {
        let InvocationResultMembershipState::Exact { by_key, key_bytes } = &self.state else {
            return;
        };
        let count = by_key.len();
        if count == 0 {
            return;
        }
        let exact_bytes = key_bytes
            .saturating_add(count.saturating_mul(INVOCATION_RESULT_MAP_ENTRY_OVERHEAD_BYTES));
        let retained = count.min(self.capacity);
        let average_key_bytes = key_bytes.div_ceil(count);
        let indexed_bytes =
            self.bloom_bits
                .div_ceil(u8::BITS as usize)
                .saturating_add(retained.saturating_mul(2usize.saturating_mul(
                    INVOCATION_RESULT_MAP_ENTRY_OVERHEAD_BYTES.saturating_add(average_key_bytes),
                )));
        if exact_bytes <= indexed_bytes {
            return;
        }

        let mut bloom = InvocationResultBloom::new(self.bloom_bits, self.bloom_hashes);
        let mut by_index: Vec<_> = by_key
            .iter()
            .map(|(key, index)| {
                bloom.insert(key);
                (*index, key.clone())
            })
            .collect();
        by_index.sort_unstable_by_key(|(index, _)| *index);
        let mut recent_by_key = OrdMap::new();
        let mut recent_by_index = OrdMap::new();
        for (index, key) in by_index.into_iter().rev().take(self.capacity) {
            recent_by_key.insert(key.clone(), index);
            recent_by_index.insert(index, key);
        }
        self.state = InvocationResultMembershipState::Indexed {
            recent_by_key,
            recent_by_index,
            bloom,
        };
    }

    fn truncate_recent(
        capacity: usize,
        recent_by_key: &mut OrdMap<IdempotencyKey, OplogIndex>,
        recent_by_index: &mut OrdMap<OplogIndex, IdempotencyKey>,
    ) {
        while recent_by_key.len() > capacity {
            let Some((oldest_index, oldest_key)) = recent_by_index
                .get_min()
                .map(|(index, key)| (*index, key.clone()))
            else {
                break;
            };
            recent_by_index.remove(&oldest_index);
            recent_by_key.remove(&oldest_key);
        }
    }
}

impl Default for InvocationResultMembership {
    fn default() -> Self {
        Self::new(
            DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY,
            DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
            DEFAULT_INVOCATION_RESULT_BLOOM_HASHES,
        )
    }
}

/// Contains status information about a worker according to a given oplog index.
///
/// This status is just cached information, all fields must be computable by the oplog alone.
/// By having an associated oplog_idx, the cached information can be used together with the
/// tail of the oplog to determine the actual status of the worker.
#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct AgentStatusRecord {
    pub status: AgentStatus,
    pub skipped_regions: DeletedRegions,
    pub overridden_retry_config: Option<RetryConfig>,
    pub pending_invocations: Vec<PendingInvocationRef>,
    pub pending_card_events: Vec<PendingCardEventRef>,
    pub pending_updates: VecDeque<PendingUpdateRef>,
    pub failed_updates: Vec<FailedUpdateRecord>,
    pub successful_updates: Vec<SuccessfulUpdateRecord>,
    pub invocation_results: InvocationResultMembership,
    pub received_card_transfers: ReceivedCardTransferIndex,
    pub durable_stream_sessions: DurableStreamSessionIndex,
    pub has_durable_stream_history: bool,
    pub current_idempotency_key: Option<IdempotencyKey>,
    pub cancelled_idempotency_key: Option<IdempotencyKey>,
    pub component_revision: ComponentRevision,
    pub component_size: u64,
    pub total_linear_memory_size: u64,
    pub owned_resources: HashMap<AgentResourceId, AgentResourceDescription>,
    pub oplog_idx: OplogIndex,
    pub active_plugins: HashSet<EnvironmentPluginGrantId>,
    pub oplog_processor_checkpoints:
        HashMap<EnvironmentPluginGrantId, OplogProcessorCheckpointState>,
    pub revoked_cards: HashSet<CardId>,
    pub deleted_regions: DeletedRegions,
    /// The component version at the starting point of the replay. Will be the version of the Create oplog entry
    /// if only automatic updates were used or the version of the latest snapshot-based update
    pub component_revision_for_replay: ComponentRevision,
    /// Semantic retry policy state per `retry_from` oplog index.
    pub current_retry_state: HashMap<OplogIndex, RetryPolicyState>,
    /// Index of the last manual update snapshot index. Agent will call load_snapshot
    /// on this payload before starting replay.
    pub last_manual_update_snapshot_index: Option<OplogIndex>,
    /// Index of the last automatic snapshot index. Must be >= last_manual_snapshot_index.
    /// Agent will call load_snapshot on this payload before starting replay. If the load_snapshot
    /// fails this will be ignored and a full replay from last_manual_snapshot_index will performed.
    pub last_automatic_snapshot_index: Option<OplogIndex>,
    /// Timestamp of the last automatic snapshot entry in the oplog.
    pub last_automatic_snapshot_timestamp: Option<Timestamp>,
    /// Component revision that created the last automatic snapshot.
    pub last_automatic_snapshot_component_revision: Option<ComponentRevision>,
    /// The agent mode the worker was created with. Decided at create time and persisted in the
    /// `Create` oplog entry; immutable for the life of the worker. `#[transient]`: it is not part
    /// of the serialized record (it is persisted separately) and defaults to `Durable` on
    /// deserialization, so readers must restore it from its own source.
    #[transient(AgentMode::Durable)]
    pub agent_mode: AgentMode,
}

impl Default for AgentStatusRecord {
    fn default() -> Self {
        AgentStatusRecord {
            status: AgentStatus::Idle,
            skipped_regions: DeletedRegions::new(),
            overridden_retry_config: None,
            pending_invocations: Vec::new(),
            pending_card_events: Vec::new(),
            pending_updates: VecDeque::new(),
            failed_updates: Vec::new(),
            successful_updates: Vec::new(),
            invocation_results: InvocationResultMembership::default(),
            received_card_transfers: ReceivedCardTransferIndex::default(),
            durable_stream_sessions: DurableStreamSessionIndex::default(),
            has_durable_stream_history: false,
            current_idempotency_key: None,
            cancelled_idempotency_key: None,
            component_revision: ComponentRevision::INITIAL,
            component_size: 0,
            total_linear_memory_size: 0,
            owned_resources: HashMap::new(),
            oplog_idx: OplogIndex::default(),
            active_plugins: HashSet::new(),
            oplog_processor_checkpoints: HashMap::new(),
            revoked_cards: HashSet::new(),
            deleted_regions: DeletedRegions::new(),
            component_revision_for_replay: ComponentRevision::INITIAL,
            current_retry_state: HashMap::new(),
            last_manual_update_snapshot_index: None,
            last_automatic_snapshot_index: None,
            last_automatic_snapshot_timestamp: None,
            last_automatic_snapshot_component_revision: None,
            agent_mode: AgentMode::Durable,
        }
    }
}

/// The durable target-side identity associated with a permission-card transfer ID.
///
/// This is stored behind an `Arc` in [`ReceivedCardTransferIndex`]. Boxing the common `Received`
/// payload separately would add an allocation and pointer indirection to every indexed transfer.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub enum ReceivedCardTransferState {
    Received {
        source_card_id: Option<CardId>,
        card: StoredCard,
    },
    Conflict,
}

/// An oplog-derived, sticky index of target-side permission-card receipts.
///
/// Receipts remain indexed even when their oplog entries are in skipped or deleted regions, so a
/// retry cannot redeliver a transfer that the target has already observed. The persistent map
/// keeps status clones and delta comparisons proportional to the changed paths rather than the
/// total number of transfers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceivedCardTransferIndex(OrdMap<Uuid, Arc<ReceivedCardTransferState>>);

impl ReceivedCardTransferIndex {
    pub fn get(&self, transfer_id: &Uuid) -> Option<&ReceivedCardTransferState> {
        self.0.get(transfer_id).map(Arc::as_ref)
    }

    pub fn insert(&mut self, transfer_id: Uuid, state: ReceivedCardTransferState) {
        self.0.insert(transfer_id, Arc::new(state));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Uuid, &ReceivedCardTransferState)> {
        self.0
            .iter()
            .map(|(transfer_id, state)| (transfer_id, state.as_ref()))
    }

    pub fn changes_from<'a>(
        &'a self,
        previous: &'a Self,
    ) -> impl Iterator<Item = (Uuid, Option<&'a ReceivedCardTransferState>)> + 'a {
        use im::ordmap::DiffItem;

        previous.0.diff(&self.0).map(|change| match change {
            DiffItem::Add(transfer_id, state) => (*transfer_id, Some(state.as_ref())),
            DiffItem::Update {
                new: (transfer_id, state),
                ..
            } => (*transfer_id, Some(state.as_ref())),
            DiffItem::Remove(transfer_id, _) => (*transfer_id, None),
        })
    }
}

impl BinarySerializer for ReceivedCardTransferIndex {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::serialize_iterator(&mut self.0.iter(), context)
    }
}

impl BinaryDeserializer for ReceivedCardTransferIndex {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let entries =
            desert_rust::deserialize_iterator::<(Uuid, Arc<ReceivedCardTransferState>)>(context)
                .0
                .collect::<desert_rust::Result<OrdMap<_, _>>>()?;
        Ok(Self(entries))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct DurableStreamSessionStatus {
    pub first_prepared: Option<OplogIndex>,
    pub prepared: Option<OplogIndex>,
    pub invocation_result: Option<OplogIndex>,
    pub finished: Option<OplogIndex>,
    pub session_key: Option<crate::model::durable_stream::StreamSessionKeyV1>,
    pub prepared_attempt_id: Option<crate::model::durable_stream::AttemptId>,
    pub initial_attachment_epoch: Option<u64>,
    pub initial_attachment_attempt_id: Option<crate::model::durable_stream::AttemptId>,
    pub initial_pending_invocation_oplog_index: Option<OplogIndex>,
    /// The referenced pending invocation after its oplog index and idempotency key were verified.
    pub validated_initial_pending_invocation: Option<OplogIndex>,
    pub attachment_epoch: Option<u64>,
    pub attachment_attempt_id: Option<crate::model::durable_stream::AttemptId>,
    pub attachment_attached: Option<bool>,
    pub lifecycle_error: Option<String>,
}

impl DurableStreamSessionStatus {
    fn invalidate_initial_attachment(&mut self) {
        self.lifecycle_error = Some(
            "durable Attached record does not identify an ordered Prepared and pending invocation"
                .into(),
        );
    }

    pub fn validate_initial_attachment_reference(
        &mut self,
        attached_idx: OplogIndex,
        attached: &crate::model::durable_stream::StreamSessionAttachedRecordV1,
    ) -> bool {
        if self.lifecycle_error.is_some() {
            return false;
        }
        let valid = attached.format_version == 1
            && self.session_key.as_ref() == Some(&attached.session_key)
            && self.prepared_attempt_id == Some(attached.attempt_id)
            && self.prepared.is_some_and(|prepared_idx| {
                prepared_idx < attached.pending_invocation_oplog_index
                    && attached.pending_invocation_oplog_index < attached_idx
            });
        if !valid {
            self.invalidate_initial_attachment();
        }
        valid
    }

    pub fn apply_pending_invocation(
        &mut self,
        oplog_idx: OplogIndex,
        idempotency_key: &IdempotencyKey,
    ) {
        if self.lifecycle_error.is_some()
            || self
                .session_key
                .as_ref()
                .is_none_or(|key| &key.idempotency_key != idempotency_key)
            || self
                .prepared
                .is_none_or(|prepared_idx| oplog_idx <= prepared_idx)
            || self.initial_attachment_epoch.is_some()
        {
            return;
        }
        if self.validated_initial_pending_invocation.is_some() {
            return;
        }
        self.validated_initial_pending_invocation = Some(oplog_idx);
    }

    pub fn apply_record(
        &mut self,
        oplog_idx: OplogIndex,
        record: &crate::model::durable_stream::StreamSessionRecordV1,
    ) {
        use crate::model::durable_stream::StreamSessionRecordV1;

        let record_key = match record {
            StreamSessionRecordV1::Prepared(v) => Some(&v.attempt.session_key),
            StreamSessionRecordV1::Attached(v) => Some(&v.session_key),
            StreamSessionRecordV1::ResumeAttempt(v) => Some(&v.attempt.session_key),
            StreamSessionRecordV1::Detached(v) => Some(&v.session_key),
            StreamSessionRecordV1::InvocationResult(v) => Some(&v.session_key),
            StreamSessionRecordV1::Finished(v) => Some(&v.session_key),
            _ => None,
        };
        let Some(record_key) = record_key else { return };
        if self
            .session_key
            .as_ref()
            .is_some_and(|key| key != record_key)
        {
            return;
        }
        self.session_key.get_or_insert_with(|| record_key.clone());
        if self.lifecycle_error.is_some() {
            return;
        }
        if !record.has_supported_format() {
            self.lifecycle_error =
                Some("unsupported or malformed durable Stream Session record version".into());
            return;
        }
        match record {
            StreamSessionRecordV1::Prepared(v) => {
                if self.prepared.is_some() {
                    self.lifecycle_error =
                        Some("durable Stream Session contains multiple Prepared records".into());
                } else {
                    self.first_prepared = Some(oplog_idx);
                    self.prepared = Some(oplog_idx);
                    self.prepared_attempt_id = Some(v.attempt.attempt_id);
                }
            }
            StreamSessionRecordV1::Attached(v) => {
                if self.initial_attachment_epoch.is_some() {
                    self.lifecycle_error =
                        Some("durable session contains a repeated initial attachment".into());
                } else if self.validate_initial_attachment_reference(oplog_idx, v) {
                    self.initial_attachment_epoch = Some(v.epoch);
                    self.initial_attachment_attempt_id = Some(v.attempt_id);
                    self.initial_pending_invocation_oplog_index =
                        Some(v.pending_invocation_oplog_index);
                    if self.validated_initial_pending_invocation
                        == Some(v.pending_invocation_oplog_index)
                    {
                        self.attachment_epoch = Some(v.epoch);
                        self.attachment_attempt_id = Some(v.attempt_id);
                        self.attachment_attached = Some(true);
                    } else {
                        self.invalidate_initial_attachment();
                    }
                }
            }
            StreamSessionRecordV1::ResumeAttempt(v) => {
                let Some(epoch) = self.attachment_epoch else {
                    self.lifecycle_error =
                        Some("durable resume precedes initial attachment".into());
                    return;
                };
                if v.attempt.expected_epoch != epoch
                    || epoch.checked_add(1) != Some(v.accepted_epoch)
                {
                    self.lifecycle_error =
                        Some("durable resume contains an invalid epoch transition".into());
                } else {
                    self.attachment_epoch = Some(v.accepted_epoch);
                    self.attachment_attempt_id = Some(v.attempt.attempt_id);
                    self.attachment_attached = Some(true);
                }
            }
            StreamSessionRecordV1::Detached(v) => {
                match (self.attachment_epoch, self.attachment_attempt_id) {
                    (None, _) => {
                        self.lifecycle_error =
                            Some("durable detach precedes initial attachment".into())
                    }
                    (Some(epoch), Some(owner))
                        if epoch == v.epoch && owner == v.owner_attempt_id =>
                    {
                        self.attachment_attached = Some(false);
                    }
                    _ => {
                        self.lifecycle_error =
                            Some("durable detach does not match the current attachment".into())
                    }
                }
            }
            StreamSessionRecordV1::InvocationResult(_) => self.invocation_result = Some(oplog_idx),
            StreamSessionRecordV1::Finished(_) => {
                self.finished.get_or_insert(oplog_idx);
            }
            _ => {}
        };
    }
}

pub const DURABLE_STREAM_SESSION_RECENT_CAPACITY: usize = 128;

/// An oplog-derived index of unfinished sessions and a bounded set of recent completions.
/// Values contain only oplog indices; canonical invocation and result payloads remain in the oplog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DurableStreamSessionIndex {
    sessions: OrdMap<String, Arc<DurableStreamSessionStatus>>,
    /// True once this status has observed local session lifecycle history. A cache miss is therefore
    /// not evidence that an older, completed session never existed.
    has_history: bool,
}

impl DurableStreamSessionIndex {
    pub fn get(&self, key: &IdempotencyKey) -> Option<&DurableStreamSessionStatus> {
        self.sessions.get(&key.value).map(Arc::as_ref)
    }

    pub fn apply_oplog_entry(
        &mut self,
        index: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<(), String> {
        use crate::model::oplog::OplogPayload;

        if let OplogEntry::PendingAgentInvocation {
            idempotency_key, ..
        } = entry
        {
            if let Some(status) = self.get(idempotency_key).cloned() {
                let mut status = status;
                status.apply_pending_invocation(index, idempotency_key);
                self.insert(idempotency_key.clone(), status);
            }
            return Ok(());
        }
        let OplogEntry::StreamSession { record, .. } = entry else {
            return Ok(());
        };
        let decoded;
        let record = match record {
            OplogPayload::Inline(record) => record.as_ref(),
            OplogPayload::SerializedInline {
                cached: Some(record),
                ..
            }
            | OplogPayload::External {
                cached: Some(record),
                ..
            } => record.as_ref(),
            OplogPayload::SerializedInline {
                bytes,
                cached: None,
            } => {
                decoded = crate::serialization::try_deserialize(bytes)
                    .map_err(|error| {
                        format!("failed to decode inline durable stream session record: {error}")
                    })?
                    .ok_or_else(|| {
                        "failed to decode inline durable stream session record: unsupported serialization version"
                            .to_string()
                    })?;
                &decoded
            }
            OplogPayload::External { cached: None, .. } => {
                return Err("durable stream session record payload has not been loaded".into());
            }
        };
        self.apply_record(index, record);
        Ok(())
    }

    pub fn apply_record(
        &mut self,
        index: OplogIndex,
        record: &crate::model::durable_stream::StreamSessionRecordV1,
    ) {
        use crate::model::durable_stream::StreamSessionRecordV1;

        let key = match record {
            StreamSessionRecordV1::Prepared(v) => &v.attempt.session_key.idempotency_key,
            StreamSessionRecordV1::Attached(v) => &v.session_key.idempotency_key,
            StreamSessionRecordV1::ResumeAttempt(v) => &v.attempt.session_key.idempotency_key,
            StreamSessionRecordV1::Detached(v) => &v.session_key.idempotency_key,
            StreamSessionRecordV1::InvocationResult(v) => &v.session_key.idempotency_key,
            StreamSessionRecordV1::Finished(v) => &v.session_key.idempotency_key,
            _ => return,
        };
        let mut status = match self.get(key) {
            Some(status) => status.clone(),
            None if matches!(record, StreamSessionRecordV1::Prepared(_)) => Default::default(),
            // Caller-side results have no local Prepared/Finished lifecycle.
            None => return,
        };
        status.apply_record(index, record);
        self.insert(key.clone(), status);
    }

    pub fn insert(&mut self, key: IdempotencyKey, status: DurableStreamSessionStatus) {
        self.has_history = true;
        let finished = status.finished.is_some();
        self.sessions.insert(key.value, Arc::new(status));
        if finished {
            self.trim_completed();
        }
    }

    pub fn has_history(&self) -> bool {
        self.has_history
    }

    fn trim_completed(&mut self) {
        let mut completed: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(key, status)| status.finished.map(|finished| (finished, key.clone())))
            .collect();
        if completed.len() > DURABLE_STREAM_SESSION_RECENT_CAPACITY {
            completed.sort_unstable();
            let excess = completed.len() - DURABLE_STREAM_SESSION_RECENT_CAPACITY;
            for (_, key) in completed.into_iter().take(excess) {
                self.sessions.remove(&key);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (IdempotencyKey, &DurableStreamSessionStatus)> {
        self.sessions
            .iter()
            .map(|(key, status)| (IdempotencyKey::new(key.clone()), status.as_ref()))
    }
}

impl BinarySerializer for DurableStreamSessionIndex {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        BinarySerializer::serialize(&self.has_history, context)?;
        desert_rust::serialize_iterator(&mut self.sessions.iter(), context)
    }
}

impl BinaryDeserializer for DurableStreamSessionIndex {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let has_history = <bool as BinaryDeserializer>::deserialize(context)?;
        let entries =
            desert_rust::deserialize_iterator::<(String, Arc<DurableStreamSessionStatus>)>(context)
                .0
                .collect::<desert_rust::Result<OrdMap<_, _>>>()?;
        Ok(Self {
            sessions: entries,
            has_history,
        })
    }
}

impl AgentStatusRecord {
    pub fn has_pending_work(&self) -> bool {
        !self.pending_invocations.is_empty() || !self.pending_updates.is_empty()
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

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct OplogProcessorCheckpointState {
    pub target_agent_id: Option<AgentId>,
    pub confirmed_up_to: OplogIndex,
    pub sending_up_to: OplogIndex,
    pub last_batch_start: OplogIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentInvocationKind {
    AgentInitialization,
    AgentMethod,
    ManualUpdate,
    LoadSnapshot,
    SaveSnapshot,
    ProcessOplogEntries,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocation {
    ManualUpdate {
        target_revision: ComponentRevision,
    },
    AgentInitialization {
        idempotency_key: IdempotencyKey,
        input: SchemaValue,
        invocation_context: InvocationContextStack,
        principal: Principal,
    },
    AgentMethod {
        idempotency_key: IdempotencyKey,
        method_name: String,
        input: SchemaValue,
        invocation_context: InvocationContextStack,
        principal: Principal,
        scope_card: Option<ScopeCard>,
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
        metadata: AgentMetadataForGuests,
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
        input: SchemaValue,
        principal: Principal,
    },
    AgentMethod {
        method_name: String,
        input: SchemaValue,
        principal: Principal,
        scope_card: Option<ScopeCard>,
    },
    LoadSnapshot {
        snapshot: RawSnapshotData,
    },
    SaveSnapshot,
    ProcessOplogEntries {
        account_id: AccountId,
        config: Vec<(String, String)>,
        metadata: AgentMetadataForGuests,
        first_entry_index: OplogIndex,
        entries: Vec<crate::model::oplog::OplogEntry>,
    },
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocationResult {
    AgentInitialization,
    AgentMethod { output: SchemaValue },
    ManualUpdate,
    LoadSnapshot { error: Option<String> },
    SaveSnapshot { snapshot: RawSnapshotData },
    ProcessOplogEntries { error: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentInvocationOutput {
    pub result: AgentInvocationResult,
    pub consumed_fuel: Option<u64>,
    pub invocation_status: Option<InvocationStatus>,
    pub component_revision: Option<ComponentRevision>,
    /// Final target of this invocation. Ephemeral invocations include their
    /// generated one-shot phantom ID. `None` when produced by a legacy executor
    /// or within the worker runtime before transport metadata is attached.
    pub agent_id: Option<AgentId>,
    /// Final idempotency key for this invocation. `None` when produced by a
    /// legacy executor or within the worker runtime before transport metadata
    /// is attached.
    pub idempotency_key: Option<IdempotencyKey>,
    /// Oplog index of the agent right after this invocation completed. `None`
    /// for synthetic outputs (e.g. lookup-only status responses) or for legacy
    /// executors that did not report it.
    pub oplog_index: Option<OplogIndex>,
    /// Per-instance fingerprint of the agent that produced this invocation
    /// result. `None` for synthetic outputs (e.g. lookup-only status responses)
    /// or for legacy executors that did not report it.
    pub agent_fingerprint: Option<AgentFingerprint>,
}

/// Compares two schema-native values for replay equivalence. This is the same
/// as structural equality except that `NaN` floats compare equal (so that a
/// replayed invocation result carrying a `NaN` is not spuriously reported as
/// diverging).
fn schema_value_replay_equivalent(a: &SchemaValue, b: &SchemaValue) -> bool {
    fn opt_box_equiv(a: &Option<Box<SchemaValue>>, b: &Option<Box<SchemaValue>>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => schema_value_replay_equivalent(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    fn slice_equiv(xs: &[SchemaValue], ys: &[SchemaValue]) -> bool {
        xs.len() == ys.len()
            && xs
                .iter()
                .zip(ys.iter())
                .all(|(x, y)| schema_value_replay_equivalent(x, y))
    }

    match (a, b) {
        (SchemaValue::F32(x), SchemaValue::F32(y)) => (x.is_nan() && y.is_nan()) || x == y,
        (SchemaValue::F64(x), SchemaValue::F64(y)) => (x.is_nan() && y.is_nan()) || x == y,
        (SchemaValue::Record { fields: xs }, SchemaValue::Record { fields: ys })
        | (SchemaValue::Tuple { elements: xs }, SchemaValue::Tuple { elements: ys })
        | (SchemaValue::List { elements: xs }, SchemaValue::List { elements: ys })
        | (SchemaValue::FixedList { elements: xs }, SchemaValue::FixedList { elements: ys }) => {
            slice_equiv(xs, ys)
        }
        (SchemaValue::Map { entries: xs }, SchemaValue::Map { entries: ys }) => {
            xs.len() == ys.len()
                && xs.iter().zip(ys.iter()).all(|((xk, xv), (yk, yv))| {
                    schema_value_replay_equivalent(xk, yk) && schema_value_replay_equivalent(xv, yv)
                })
        }
        (SchemaValue::Variant(a), SchemaValue::Variant(b)) => {
            a.case == b.case && opt_box_equiv(&a.payload, &b.payload)
        }
        (SchemaValue::Option { inner: a }, SchemaValue::Option { inner: b }) => opt_box_equiv(a, b),
        (SchemaValue::Result(a), SchemaValue::Result(b)) => match (a, b) {
            (ResultValuePayload::Ok { value: a }, ResultValuePayload::Ok { value: b })
            | (ResultValuePayload::Err { value: a }, ResultValuePayload::Err { value: b }) => {
                opt_box_equiv(a, b)
            }
            _ => false,
        },
        (SchemaValue::Union(a), SchemaValue::Union(b)) => {
            a.tag == b.tag && schema_value_replay_equivalent(&a.body, &b.body)
        }
        _ => a == b,
    }
}

impl AgentInvocationResult {
    pub fn replay_equivalent(&self, other: &AgentInvocationResult) -> bool {
        match (self, other) {
            (
                AgentInvocationResult::AgentInitialization,
                AgentInvocationResult::AgentInitialization,
            ) => true,
            (
                AgentInvocationResult::AgentMethod { output: a },
                AgentInvocationResult::AgentMethod { output: b },
            ) => schema_value_replay_equivalent(a, b),
            (AgentInvocationResult::ManualUpdate, AgentInvocationResult::ManualUpdate) => true,
            (
                AgentInvocationResult::LoadSnapshot { error: a },
                AgentInvocationResult::LoadSnapshot { error: b },
            ) => a == b,
            (
                AgentInvocationResult::SaveSnapshot { snapshot: a },
                AgentInvocationResult::SaveSnapshot { snapshot: b },
            ) => a == b,
            (
                AgentInvocationResult::ProcessOplogEntries { error: a },
                AgentInvocationResult::ProcessOplogEntries { error: b },
            ) => a == b,
            _ => false,
        }
    }

    /// Wraps this result for `Debug` rendering that redacts any host-managed
    /// capability material (`Secret` / `QuotaToken`) in the embedded
    /// invocation output. Use at tracing / diagnostic / error-formatting
    /// boundaries instead of the derived `Debug`.
    pub fn redacted_debug(&self) -> RedactedAgentInvocationResult<'_> {
        RedactedAgentInvocationResult(self)
    }
}

/// `Debug` wrapper produced by [`AgentInvocationResult::redacted_debug`].
pub struct RedactedAgentInvocationResult<'a>(&'a AgentInvocationResult);

impl std::fmt::Debug for RedactedAgentInvocationResult<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            AgentInvocationResult::AgentMethod { output } => f
                .debug_struct("AgentMethod")
                .field(
                    "output",
                    &crate::schema::redacted_schema_value_debug(output),
                )
                .finish(),
            other => std::fmt::Debug::fmt(other, f),
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
            AgentInvocationPayload::AgentInitialization { input, principal } => {
                Self::AgentInitialization {
                    idempotency_key,
                    input,
                    invocation_context,
                    principal,
                }
            }
            AgentInvocationPayload::AgentMethod {
                method_name,
                input,
                principal,
                scope_card,
            } => Self::AgentMethod {
                idempotency_key,
                method_name,
                input,
                invocation_context,
                principal,
                scope_card,
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
                principal,
            } => (
                idempotency_key,
                AgentInvocationPayload::AgentInitialization { input, principal },
                invocation_context,
            ),
            Self::AgentMethod {
                idempotency_key,
                method_name,
                input,
                invocation_context,
                principal,
                scope_card,
            } => (
                idempotency_key,
                AgentInvocationPayload::AgentMethod {
                    method_name,
                    input,
                    principal,
                    scope_card,
                },
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
        self.idempotency_key() == Some(key)
    }

    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        match self {
            Self::AgentMethod {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::AgentInitialization {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::ProcessOplogEntries {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::SaveSnapshot { idempotency_key } => Some(idempotency_key),
            Self::LoadSnapshot {
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

    pub fn kind(&self) -> AgentInvocationKind {
        match self {
            Self::ManualUpdate { .. } => AgentInvocationKind::ManualUpdate,
            Self::AgentInitialization { .. } => AgentInvocationKind::AgentInitialization,
            Self::AgentMethod { .. } => AgentInvocationKind::AgentMethod,
            Self::LoadSnapshot { .. } => AgentInvocationKind::LoadSnapshot,
            Self::SaveSnapshot { .. } => AgentInvocationKind::SaveSnapshot,
            Self::ProcessOplogEntries { .. } => AgentInvocationKind::ProcessOplogEntries,
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::ManualUpdate { .. } => String::new(),
            Self::AgentInitialization { .. } => "initialize".to_string(),
            Self::AgentMethod { method_name, .. } => method_name.clone(),
            Self::LoadSnapshot { .. } => "load-snapshot".to_string(),
            Self::SaveSnapshot { .. } => "save-snapshot".to_string(),
            Self::ProcessOplogEntries { .. } => "process-oplog-entries".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct TimestampedAgentInvocation {
    pub timestamp: Timestamp,
    pub invocation: AgentInvocation,
}

/// A lightweight reference to a pending agent invocation whose full payload is stored in the
/// oplog.
///
/// The complete invocation (input parameters, snapshot data, oplog entry batches, ...) lives
/// in the `PendingAgentInvocation` oplog entry at `oplog_index`. The status record only keeps
/// the minimal routing metadata that consumers need without executing the invocation. Paths
/// that actually run the invocation hydrate the full [`TimestampedAgentInvocation`] from the
/// oplog on demand.
#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct PendingInvocationRef {
    pub timestamp: Timestamp,
    /// Index of the `PendingAgentInvocation` oplog entry holding the full payload.
    pub oplog_index: OplogIndex,
    /// Semantic idempotency key of the invocation. `None` for manual updates.
    pub idempotency_key: Option<IdempotencyKey>,
    /// Target revision of the manual update. `Some` only for manual update invocations.
    pub manual_update_target_revision: Option<ComponentRevision>,
}

impl PendingInvocationRef {
    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }

    pub fn has_idempotency_key(&self, key: &IdempotencyKey) -> bool {
        self.idempotency_key.as_ref() == Some(key)
    }

    pub fn is_manual_update(&self) -> bool {
        self.manual_update_target_revision.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct PendingCardEventRef {
    pub timestamp: Timestamp,
    pub oplog_index: OplogIndex,
    pub event: QueuedCardEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum PendingUpdateKind {
    Automatic,
    SnapshotBased,
}

/// A lightweight reference to a pending update whose full description is stored in the oplog.
///
/// The complete [`UpdateDescription`](crate::model::oplog::UpdateDescription) (including any
/// snapshot payload) lives in the `PendingUpdate` oplog entry at `oplog_index`. The status
/// record only keeps the metadata needed to schedule the update; the snapshot payload is
/// hydrated from the oplog on demand when the update is applied.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct PendingUpdateRef {
    pub timestamp: Timestamp,
    /// Index of the `PendingUpdate` oplog entry holding the full description.
    pub oplog_index: OplogIndex,
    pub target_revision: ComponentRevision,
    pub kind: PendingUpdateKind,
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
pub enum AgentEvent {
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
    PluginError {
        timestamp: Timestamp,
        plugin_name: String,
        message: String,
    },
    SnapshotRecoverySucceeded {
        timestamp: Timestamp,
        snapshot_index: OplogIndex,
    },
    SnapshotRecoveryFailed {
        timestamp: Timestamp,
        snapshot_index: OplogIndex,
        error: String,
    },
    /// The client fell behind and the point it left of is no longer in our buffer.
    /// {number_of_skipped_messages} is the number of messages between the client left of and the point it is now at.
    ClientLagged { number_of_missed_messages: u64 },
}

impl Display for AgentEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentEvent::StdOut { bytes, .. } => {
                write!(
                    f,
                    "<stdout> {}",
                    String::from_utf8(bytes.clone()).unwrap_or_default()
                )
            }
            AgentEvent::StdErr { bytes, .. } => {
                write!(
                    f,
                    "<stderr> {}",
                    String::from_utf8(bytes.clone()).unwrap_or_default()
                )
            }
            AgentEvent::Log {
                level,
                context,
                message,
                ..
            } => {
                write!(f, "<log> {level:?} {context} {message}")
            }
            AgentEvent::InvocationStart {
                function,
                idempotency_key,
                ..
            } => {
                write!(f, "<invocation-start> {function} {idempotency_key}")
            }
            AgentEvent::InvocationFinished {
                function,
                idempotency_key,
                ..
            } => {
                write!(f, "<invocation-finished> {function} {idempotency_key}")
            }
            AgentEvent::PluginError {
                plugin_name,
                message,
                ..
            } => {
                write!(f, "<plugin-error> [{plugin_name}] {message}")
            }
            AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. } => {
                write!(f, "<snapshot-recovery-succeeded> {snapshot_index}")
            }
            AgentEvent::SnapshotRecoveryFailed {
                snapshot_index,
                error,
                ..
            } => {
                write!(f, "<snapshot-recovery-failed> {snapshot_index}: {error}")
            }
            AgentEvent::ClientLagged {
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

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub enum ForkResult {
    /// The original worker that called `fork`
    Original,
    /// The new worker
    Forked,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[cfg(test)]
mod shard_assignment_tests {
    use super::{ShardAssignment, ShardDeliveryOutcome, ShardEpoch, ShardId, ShardLeaseRevision};
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::time::{Duration, Instant};
    use test_r::test;

    test_r::enable!();

    fn epochs(entries: impl IntoIterator<Item = (i64, u64)>) -> HashMap<ShardId, ShardEpoch> {
        entries
            .into_iter()
            .map(|(shard_id, epoch)| (ShardId::new(shard_id), ShardEpoch(epoch)))
            .collect()
    }

    fn in_secs(seconds: u64) -> Instant {
        Instant::now() + Duration::from_secs(seconds)
    }

    /// The push says "exactly these"; anything absent is dropped.
    #[test]
    fn set_shards_replaces_the_set_rather_than_merging_into_it() {
        let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0), ShardId::new(1)]);

        let outcome = assignment.set_shards(8, &epochs([(1, 4)]), ShardLeaseRevision(1));

        assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
        assert!(!assignment.contains(&ShardId::new(0)));
        assert_eq!(assignment.epoch_of(&ShardId::new(1)), Some(ShardEpoch(4)));
        assert_eq!(assignment.len(), 1);
        assert_eq!(assignment.revision, ShardLeaseRevision(1));
    }

    /// Two deliveries can cross on the network. A renewal reply read from an
    /// older state than a push must not undo the push when it arrives second:
    /// ordering by revision is what keeps both delivery paths safe. The lease
    /// clock still moves, because the grant answers this executor's own
    /// request, anchored before that request went out, and a push carries no
    /// lease that could have made this one redundant.
    #[test]
    fn a_stale_grant_keeps_the_set_but_still_moves_the_lease_clock() {
        let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);
        assignment.set_shards(8, &epochs([(0, 1), (5, 2)]), ShardLeaseRevision(7));

        let granted = in_secs(60);
        let outcome =
            assignment.adopt_grant(None, &epochs([(0, 1)]), granted, ShardLeaseRevision(6));

        assert_eq!(
            outcome,
            ShardDeliveryOutcome::Stale {
                delivered: ShardLeaseRevision(6),
                applied: ShardLeaseRevision(7),
            }
        );
        assert_eq!(
            assignment.shard_epochs,
            epochs([(0, 1), (5, 2)]),
            "the older delivery narrowed the set the newer one had just widened"
        );
        assert_eq!(assignment.revision, ShardLeaseRevision(7));
        assert_eq!(
            assignment.expires_at,
            Some(granted),
            "a grant's lease is about time, not about the set; a stale set must not cost the lease"
        );
    }

    /// Equal revisions come from the same persisted state and carry the same
    /// set, so a renewal at the revision of the last push applies rather than
    /// being dropped.
    #[test]
    fn a_delivery_at_the_same_revision_is_applied() {
        let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);
        assignment.set_shards(8, &epochs([(0, 1)]), ShardLeaseRevision(7));

        let refreshed = in_secs(60);
        let outcome =
            assignment.adopt_grant(None, &epochs([(0, 1)]), refreshed, ShardLeaseRevision(7));

        assert_eq!(
            outcome,
            ShardDeliveryOutcome::Applied { set_changed: false }
        );
        assert_eq!(assignment.expires_at, Some(refreshed));
    }

    /// A push has no request of this executor's to anchor a lease to, so it
    /// carries none: neither a full replace nor a revoke moves the lease clock,
    /// whatever revision they carry.
    #[test]
    fn a_push_never_moves_the_lease_clock() {
        let mut assignment = ShardAssignment::default();
        let granted = in_secs(60);
        assignment.adopt_grant(
            Some(8),
            &epochs([(0, 1), (1, 1)]),
            granted,
            ShardLeaseRevision(1),
        );

        assignment.set_shards(8, &epochs([(0, 1), (1, 1), (2, 1)]), ShardLeaseRevision(2));
        assert_eq!(
            assignment.expires_at,
            Some(granted),
            "a full-replace push must leave the lease where the grant put it"
        );

        assignment.revoke_shards(&HashSet::from([ShardId::new(2)]), ShardLeaseRevision(3));
        assert_eq!(
            assignment.expires_at,
            Some(granted),
            "a revoke must leave the lease where the grant put it"
        );
    }

    /// The lease clock is written, never maxed: a manager whose lease length was
    /// reduced across a restart holds the shorter expiry, and an executor that
    /// kept the longer one would admit work past the manager's reap.
    #[test]
    fn a_grant_with_a_shorter_lease_shortens_it() {
        let mut assignment = ShardAssignment::default();
        let now = Instant::now();
        assignment.adopt_grant(
            Some(8),
            &epochs([(0, 1)]),
            now + Duration::from_secs(60),
            ShardLeaseRevision(1),
        );

        assignment.adopt_grant(
            None,
            &epochs([(0, 1)]),
            now + Duration::from_secs(30),
            ShardLeaseRevision(2),
        );

        assert_eq!(assignment.expires_at, Some(now + Duration::from_secs(30)));
    }

    /// A revoke is a delta rather than a full set, but it is gated the same
    /// way: once it has applied, a grant read before the shard moved is older
    /// than it and cannot put the shard back.
    #[test]
    fn a_revoke_is_gated_by_revision_like_every_other_delivery() {
        let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0), ShardId::new(1)]);
        let expiry = in_secs(60);
        assignment.adopt_grant(
            Some(8),
            &epochs([(0, 1), (1, 1)]),
            expiry,
            ShardLeaseRevision(3),
        );
        let revoked = HashSet::from([ShardId::new(0)]);

        let stale = assignment.revoke_shards(&revoked, ShardLeaseRevision(2));
        assert_eq!(
            stale,
            ShardDeliveryOutcome::Stale {
                delivered: ShardLeaseRevision(2),
                applied: ShardLeaseRevision(3),
            }
        );
        assert!(
            assignment.contains(&ShardId::new(0)),
            "a revoke older than the last delivery applied must be ignored"
        );

        let applied = assignment.revoke_shards(&revoked, ShardLeaseRevision(5));
        assert_eq!(applied, ShardDeliveryOutcome::Applied { set_changed: true });
        assert!(!assignment.contains(&ShardId::new(0)));
        assert_eq!(
            assignment.revision,
            ShardLeaseRevision(5),
            "the revoke's revision is recorded like any other delivery's"
        );
        assert_eq!(
            assignment.expires_at,
            Some(expiry),
            "a revoke drops shards; it does not touch the lease"
        );

        let late_grant = assignment.adopt_grant(
            None,
            &epochs([(0, 1), (1, 1)]),
            in_secs(60),
            ShardLeaseRevision(4),
        );
        assert!(matches!(late_grant, ShardDeliveryOutcome::Stale { .. }));
        assert!(
            !assignment.contains(&ShardId::new(0)),
            "a grant read before the shard moved, arriving after the revoke, must not restore it"
        );
    }

    /// The corrective delivery: a renewal that answers with a different set
    /// than was claimed is applied like a push, and reports the set moved so
    /// the caller sweeps and recovers.
    #[test]
    fn a_renewal_that_changes_the_set_reports_it() {
        let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0), ShardId::new(1)]);
        assignment.set_shards(8, &epochs([(0, 1), (1, 1)]), ShardLeaseRevision(3));

        let outcome = assignment.adopt_grant(
            None,
            &epochs([(1, 1), (2, 5)]),
            in_secs(60),
            ShardLeaseRevision(4),
        );

        assert_eq!(outcome, ShardDeliveryOutcome::Applied { set_changed: true });
        assert!(
            !assignment.contains(&ShardId::new(0)),
            "the dropped shard is gone"
        );
        assert_eq!(assignment.epoch_of(&ShardId::new(2)), Some(ShardEpoch(5)));
        assert_eq!(assignment.revision, ShardLeaseRevision(4));
    }

    /// `clear()` lapses the lease as of `now`. `None` would mean
    /// "never expires", which would leave a fenced executor reading as ready.
    #[test]
    fn clear_lapses_the_lease_instead_of_making_it_unexpiring() {
        let mut assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);
        let now = Instant::now();
        assert!(assignment.lease_is_live(now));

        assignment.clear(now);

        assert!(assignment.is_empty());
        assert_eq!(assignment.expires_at, Some(now));
        assert!(
            !assignment.lease_is_live(now),
            "a cleared assignment is lapsed, not never-expiring"
        );
        assert!(!assignment.lease_is_live(now + Duration::from_secs(1)));
    }

    /// The single-shard implementations and the debugging service run with no
    /// expiry at all and must never fence themselves.
    #[test]
    fn a_lease_without_an_expiry_is_always_live() {
        let assignment = ShardAssignment::unexpiring(8, [ShardId::new(0)]);

        assert!(assignment.lease_is_live(Instant::now()));
        assert!(assignment.lease_is_live(in_secs(365 * 24 * 3600)));
    }
}
