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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use im::{OrdMap, Vector};
use uuid::Uuid;

use super::{AgentMode, OwnedAgentId, PendingInvocationRef};
use crate::model::account::{AccountEmail, AccountId};
use crate::model::agent::TypedAgentConfigEntry;
use crate::model::card::{CardId, ReceivedCardTransferIndex, SourceCardTransferIndex};
use crate::model::component::ComponentRevision;
use crate::model::environment::EnvironmentId;
use crate::model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::model::oplog::{
    self, AgentResourceId, FilesystemSnapshotName, OplogEntry, OplogIndex, QueuedCardEvent,
};
use crate::model::regions::DeletedRegions;
use crate::model::{
    AgentAndFilter, AgentConfigVarsFilter, AgentCreatedAtFilter, AgentEnvFilter, AgentFilter,
    AgentFingerprint, AgentId, AgentModeFilter, AgentNameFilter, AgentNotFilter, AgentOrFilter,
    AgentResourceDescription, AgentRevisionFilter, AgentStatus, AgentStatusFilter, IdempotencyKey,
    RetryConfig, RetryPolicyState, Timestamp,
};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentMetadata {
    pub agent_id: AgentId,
    pub owner_kind: crate::model::agent::OwnerKind,
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

/// The newest automatic snapshot entry in the oplog, with the filesystem snapshot that it names
/// and the confirmation of that snapshot.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct AutomaticSnapshot {
    /// The index of the `Snapshot` entry.
    pub index: OplogIndex,
    /// The time of the `Snapshot` entry.
    pub timestamp: Timestamp,
    /// The component revision that made the entry.
    pub component_revision: ComponentRevision,
    /// The filesystem snapshot that the entry names, with its confirmation.
    pub files: SnapshotFiles,
}

/// The filesystem snapshot that an automatic snapshot entry names, with its confirmation. Only a
/// named snapshot can be confirmed.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub enum SnapshotFiles {
    /// The entry names no filesystem snapshot: it has no filesystem capture.
    Unnamed,
    /// The entry names this filesystem snapshot, and no `SnapshotConfirmed` entry confirmed it.
    Unconfirmed(FilesystemSnapshotName),
    /// A `SnapshotConfirmed` entry with the same name follows the entry.
    Confirmed(FilesystemSnapshotName),
}

impl SnapshotFiles {
    /// The files of a new entry that names `name`, before any confirmation.
    pub fn named(name: Option<FilesystemSnapshotName>) -> Self {
        name.map_or(Self::Unnamed, Self::Unconfirmed)
    }

    /// The name of the filesystem snapshot, when the entry names one.
    pub fn name(&self) -> Option<&FilesystemSnapshotName> {
        match self {
            Self::Unnamed => None,
            Self::Unconfirmed(name) | Self::Confirmed(name) => Some(name),
        }
    }

    /// The files after a `SnapshotConfirmed` entry of `name`: confirmed when the entry names
    /// `name` and is unconfirmed, and unchanged otherwise.
    pub fn confirmed(self, name: &FilesystemSnapshotName) -> Self {
        match self {
            Self::Unconfirmed(own) if &own == name => Self::Confirmed(own),
            files => files,
        }
    }
}

impl AutomaticSnapshot {
    /// The entry as a baseline of a start, when it is usable: its filesystem snapshot is
    /// confirmed, or it names none.
    pub fn into_usable(self) -> Option<UsableAutomaticSnapshot> {
        let filesystem_snapshot = match self.files {
            SnapshotFiles::Unnamed => None,
            SnapshotFiles::Confirmed(name) => Some(name),
            SnapshotFiles::Unconfirmed(_) => return None,
        };
        Some(UsableAutomaticSnapshot {
            index: self.index,
            component_revision: self.component_revision,
            filesystem_snapshot,
        })
    }
}

/// An automatic snapshot entry that a start can use as its baseline: its filesystem snapshot is
/// confirmed, or it has no filesystem snapshot name.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct UsableAutomaticSnapshot {
    /// The index of the `Snapshot` entry.
    pub index: OplogIndex,
    /// The component revision that made the entry.
    pub component_revision: ComponentRevision,
    /// The filesystem snapshot name of the entry, which a `SnapshotConfirmed` entry confirmed.
    /// `None` when the entry has no filesystem snapshot name.
    pub filesystem_snapshot: Option<FilesystemSnapshotName>,
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
    pub last_error_kind: Option<crate::base_model::oplog::OplogErrorKind>,
    pub skipped_regions: DeletedRegions,
    pub overridden_retry_config: Option<RetryConfig>,
    pub pending_invocations: Vec<PendingInvocationRef>,
    pub pending_card_events: Vec<PendingCardEventRef>,
    pub pending_updates: VecDeque<PendingUpdateRef>,
    pub failed_updates: Vec<FailedUpdateRecord>,
    pub successful_updates: Vec<SuccessfulUpdateRecord>,
    pub invocation_results: InvocationResultMembership,
    pub received_card_transfers: ReceivedCardTransferIndex,
    pub source_card_transfers: SourceCardTransferIndex,
    pub entity_replay_progress: EntityReplayProgress,
    pub durable_stream_sessions: DurableStreamSessionIndex,
    pub export_fork_admissions: ExportForkAdmissions,
    pub has_durable_stream_history: bool,
    pub pending_durable_stream_cancellations:
        HashSet<crate::model::durable_stream::StreamConsumerCancelIntentRecord>,
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
    /// Historical component revision used for replay metadata at the recovery baseline.
    /// Initially the `Create` revision; successful manual snapshot updates set it to their target
    /// revision, and successful snapshot-assisted automatic updates set it to their source revision.
    /// Automatic updates without snapshot assistance leave it unchanged.
    pub component_revision_for_replay: ComponentRevision,
    /// Oplog index that established the active component revision (`Create` or the latest
    /// surviving `SuccessfulUpdate`).
    pub component_revision_start_index: OplogIndex,
    /// Semantic retry policy state per `retry_from` oplog index.
    pub current_retry_state: HashMap<OplogIndex, RetryPolicyState>,
    /// Mandatory recovery snapshot established by a successful update. Agent will call
    /// load_snapshot on this payload before starting replay.
    pub authoritative_snapshot: Option<AuthoritativeSnapshot>,
    /// The last automatic snapshot entry. Its index is after the index of `authoritative_snapshot`.
    /// A start that selects it calls load_snapshot on its payload before it starts the replay. If
    /// the load_snapshot fails, the start rejects this entry and tries
    /// `previous_usable_automatic_snapshot`, then `authoritative_snapshot`, and a full replay only
    /// when there is no authoritative snapshot.
    pub last_automatic_snapshot: Option<AutomaticSnapshot>,
    /// The newest automatic snapshot entry before the last one that was usable when the entry
    /// after it came: an entry with a confirmed filesystem snapshot, or an entry without a
    /// filesystem snapshot name. A new entry moves a usable last entry here, also when it reuses
    /// the filesystem snapshot name of that entry, because each entry has its own application
    /// snapshot. A start uses this entry when the last automatic snapshot entry is not usable, or
    /// when the last entry does not load or restore. A successful update clears it together with
    /// the last automatic snapshot entry.
    pub previous_usable_automatic_snapshot: Option<UsableAutomaticSnapshot>,
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
            last_error_kind: None,
            skipped_regions: DeletedRegions::new(),
            overridden_retry_config: None,
            pending_invocations: Vec::new(),
            pending_card_events: Vec::new(),
            pending_updates: VecDeque::new(),
            failed_updates: Vec::new(),
            successful_updates: Vec::new(),
            invocation_results: InvocationResultMembership::default(),
            received_card_transfers: ReceivedCardTransferIndex::default(),
            source_card_transfers: SourceCardTransferIndex::default(),
            entity_replay_progress: EntityReplayProgress::default(),
            durable_stream_sessions: DurableStreamSessionIndex::default(),
            export_fork_admissions: ExportForkAdmissions::default(),
            has_durable_stream_history: false,
            pending_durable_stream_cancellations: HashSet::new(),
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
            component_revision_start_index: OplogIndex::INITIAL,
            current_retry_state: HashMap::new(),
            authoritative_snapshot: None,
            last_automatic_snapshot: None,
            previous_usable_automatic_snapshot: None,
            agent_mode: AgentMode::Durable,
        }
    }
}

/// Replay prerequisites strictly before an entity's terminal. Parentage follows durable call
/// nesting, never observational ownership. Later descendants cannot change a frozen boundary.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub struct EntityReplayBoundary {
    pub terminal_index: OplogIndex,
    pub last_required: Option<OplogIndex>,
    pub parent_entity: Option<OplogIndex>,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub struct OpenEntityReplayScope {
    pub last_required: Option<OplogIndex>,
    pub parent_entity: Option<OplogIndex>,
}

/// Incremental replay evidence at the enclosing status record's `oplog_idx`.
/// Routes include completed protocol parents while an enclosing entity is still open.
/// Persistent maps make status snapshots share unchanged history.
/// `completed` grows with the number of visible entity completions in the folded history,
/// not just the number of open entities. These frozen boundaries remain replay evidence after
/// scope retirement and must not be trimmed by age or invocation completion. Changes to skipped
/// or deleted regions invalidate affected fold baselines and require refolding visible history;
/// removing entries from `completed` alone cannot repair boundaries derived from hidden work.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityReplayProgress {
    pub completed: OrdMap<OplogIndex, EntityReplayBoundary>,
    pub open: OrdMap<OplogIndex, OpenEntityReplayScope>,
    pub routes: OrdMap<OplogIndex, OplogIndex>,
    /// Exact inverse of `routes`: `(enclosing entity, Start)`.
    pub route_members: OrdMap<(OplogIndex, OplogIndex), ()>,
    /// Retained entity-tree edges `(parent, child)`. Retirement removes an edge before
    /// visiting the child and stops at open children, which then own their own retirement.
    pub entity_children: OrdMap<(OplogIndex, OplogIndex), ()>,
}

impl BinarySerializer for EntityReplayProgress {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::serialize_iterator(&mut self.completed.iter(), context)?;
        desert_rust::serialize_iterator(&mut self.open.iter(), context)?;
        desert_rust::serialize_iterator(&mut self.routes.iter(), context)?;
        desert_rust::serialize_iterator(&mut self.route_members.iter(), context)?;
        desert_rust::serialize_iterator(&mut self.entity_children.iter(), context)
    }
}

impl BinaryDeserializer for EntityReplayProgress {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        Ok(Self {
            completed: desert_rust::deserialize_iterator::<(OplogIndex, EntityReplayBoundary)>(
                context,
            )
            .0
            .collect::<desert_rust::Result<_>>()?,
            open: desert_rust::deserialize_iterator::<(OplogIndex, OpenEntityReplayScope)>(context)
                .0
                .collect::<desert_rust::Result<_>>()?,
            routes: desert_rust::deserialize_iterator::<(OplogIndex, OplogIndex)>(context)
                .0
                .collect::<desert_rust::Result<_>>()?,
            route_members: desert_rust::deserialize_iterator::<((OplogIndex, OplogIndex), ())>(
                context,
            )
            .0
            .collect::<desert_rust::Result<_>>()?,
            entity_children: desert_rust::deserialize_iterator::<((OplogIndex, OplogIndex), ())>(
                context,
            )
            .0
            .collect::<desert_rust::Result<_>>()?,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, BinaryCodec)]
pub struct ExportForkAdmissions {
    pub owner_fingerprint: Option<AgentFingerprint>,
    pub reservations: HashMap<AgentId, ExportForkReservation>,
    pub session_counts: HashMap<String, u32>,
    pub updated_millis: u64,
    pub credit_millis: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub struct ExportForkReservation {
    pub oplog_index: OplogIndex,
    pub request_hash: Vec<u8>,
    pub session: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct DurableStreamSessionStatus {
    pub first_prepared: Option<OplogIndex>,
    pub prepared: Option<OplogIndex>,
    pub invocation_result: Option<OplogIndex>,
    pub finished: Option<OplogIndex>,
    pub session_key: Option<IdempotencyKey>,
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
    pub tombstoned_slots: HashSet<String>,
    pub cancellation_requested: bool,
    pub public_session_id: Option<String>,
    pub expiry_policy: crate::model::durable_stream::StreamSessionExpiryPolicy,
    pub expiry_deadline_millis: Option<u64>,
    pub expired: bool,
    pub export_fork_initialized: bool,
    pub export_source_invocation: Option<crate::model::durable_stream::StreamInvocationId>,
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
        attached: &crate::model::durable_stream::StreamSessionAttachedRecord,
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
                .is_none_or(|key| key != idempotency_key)
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
        record: &crate::model::durable_stream::StreamSessionRecord,
    ) {
        use crate::model::durable_stream::StreamSessionRecord;

        if let StreamSessionRecord::ForkCut(cut) = record {
            self.attachment_epoch = Some(cut.epoch_floor);
            self.attachment_attached = Some(false);
            if cut.revert.is_none() {
                // Concrete Prepared invocation identities remain valid continuations on an
                // ordinary fork. A target-only export identity is replaced on initialization.
                if self.first_prepared.is_none() {
                    self.public_session_id = None;
                }
                self.expiry_policy = crate::model::durable_stream::StreamSessionExpiryPolicy::None;
                self.expiry_deadline_millis = None;
                self.expired = false;
            }
            return;
        }

        let local_record_key = match record {
            StreamSessionRecord::Prepared(v) => Some(&v.session_key),
            StreamSessionRecord::ExpiryRefreshed(v) => Some(&v.session_key),
            StreamSessionRecord::Expired(v) => Some(&v.session_key),
            StreamSessionRecord::ExportForkInitialized(v) => Some(&v.session_key),
            StreamSessionRecord::Attached(v) => Some(&v.session_key),
            StreamSessionRecord::ResumeAttempt(v) => Some(&v.session_key),
            StreamSessionRecord::Detached(v) => Some(&v.session_key),
            _ => None,
        };
        let relative_record_key = match record {
            StreamSessionRecord::InvocationResult(v) => Some(&v.session_key),
            StreamSessionRecord::Finished(v) => Some(&v.session_key),
            StreamSessionRecord::Tombstoned(v) => Some(&v.session_key),
            StreamSessionRecord::CancelRequested(v) => Some(&v.session_key),
            StreamSessionRecord::ConsumerCancelApplied(v) => Some(&v.intent.session_key),
            _ => None,
        };
        if let Some(record_key) = local_record_key {
            if self
                .session_key
                .as_ref()
                .is_some_and(|key| key != record_key)
            {
                return;
            }
            self.session_key.get_or_insert_with(|| record_key.clone());
        } else if let Some(record_key) = relative_record_key {
            let Some(key) = self.session_key.as_ref() else {
                return;
            };
            if !matches!(record_key,
                crate::model::durable_stream::StreamRegistrationInvocation::Local(local)
                    if local == key)
            {
                return;
            }
        } else {
            return;
        }
        if self.lifecycle_error.is_some() {
            return;
        }
        if !record.has_supported_format() {
            self.lifecycle_error =
                Some("unsupported or malformed durable Stream Session record version".into());
            return;
        }
        match record {
            StreamSessionRecord::Prepared(v) => {
                if self.prepared.is_some() {
                    self.lifecycle_error =
                        Some("durable Stream Session contains multiple Prepared records".into());
                } else {
                    self.first_prepared = Some(oplog_idx);
                    self.prepared = Some(oplog_idx);
                    self.prepared_attempt_id = Some(v.attempt.attempt_id);
                    self.public_session_id = Some(v.public_session_id.clone());
                    self.expiry_policy = v.expiry_policy;
                    self.expiry_deadline_millis = v.expiry_deadline_millis;
                }
            }
            StreamSessionRecord::ExpiryRefreshed(v)
                if self.public_session_id.as_deref() == Some(&v.public_session_id)
                    && self.expiry_deadline_millis == Some(v.expected_deadline_millis)
                    && matches!(
                        self.expiry_policy,
                        crate::model::durable_stream::StreamSessionExpiryPolicy::Sliding { .. }
                    )
                    && !self.expired =>
            {
                self.expiry_deadline_millis = Some(v.deadline_millis);
            }
            StreamSessionRecord::Expired(v)
                if self.public_session_id.as_deref() == Some(&v.public_session_id)
                    && self.expiry_deadline_millis == Some(v.expected_deadline_millis)
                    && !self.expired =>
            {
                self.expired = true;
            }
            StreamSessionRecord::ExportForkInitialized(v) if self.public_session_id.is_none() => {
                self.public_session_id = Some(v.public_session_id.clone());
                self.expiry_policy = v.expiry_policy;
                self.expiry_deadline_millis = v.expiry_deadline_millis;
                self.expired = false;
                self.export_fork_initialized = true;
                self.export_source_invocation = Some(v.source_invocation.clone());
            }
            StreamSessionRecord::Attached(v) => {
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
            StreamSessionRecord::ResumeAttempt(v) => {
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
            StreamSessionRecord::Detached(v) => {
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
            StreamSessionRecord::InvocationResult(_) => self.invocation_result = Some(oplog_idx),
            StreamSessionRecord::Finished(_) => {
                self.finished.get_or_insert(oplog_idx);
            }
            StreamSessionRecord::Tombstoned(v) => {
                self.tombstoned_slots.insert(v.slot.clone());
            }
            StreamSessionRecord::CancelRequested(_) => self.cancellation_requested = true,
            _ => {}
        };
    }
}

pub const DURABLE_STREAM_SESSION_RECENT_CAPACITY: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub enum DurableStreamPublicBinding {
    Live {
        session_key: IdempotencyKey,
        expiry_policy: crate::model::durable_stream::StreamSessionExpiryPolicy,
        expiry_deadline_millis: Option<u64>,
    },
    Retired {
        session_key: IdempotencyKey,
    },
}

impl DurableStreamPublicBinding {
    pub fn fold(
        current: Option<&Self>,
        record: &crate::model::durable_stream::StreamSessionRecord,
    ) -> Option<Self> {
        use crate::model::durable_stream::StreamSessionRecord;

        match record {
            StreamSessionRecord::Prepared(record) => match current {
                None => Some(Self::Live {
                    session_key: record.session_key.clone(),
                    expiry_policy: record.expiry_policy,
                    expiry_deadline_millis: record.expiry_deadline_millis,
                }),
                Some(Self::Retired { session_key }) if session_key != &record.session_key => {
                    Some(Self::Live {
                        session_key: record.session_key.clone(),
                        expiry_policy: record.expiry_policy,
                        expiry_deadline_millis: record.expiry_deadline_millis,
                    })
                }
                Some(current) => Some(current.clone()),
            },
            StreamSessionRecord::ExportForkInitialized(record) => match current {
                None => Some(Self::Live {
                    session_key: record.session_key.clone(),
                    expiry_policy: record.expiry_policy,
                    expiry_deadline_millis: record.expiry_deadline_millis,
                }),
                Some(Self::Retired { session_key }) if session_key != &record.session_key => {
                    Some(Self::Live {
                        session_key: record.session_key.clone(),
                        expiry_policy: record.expiry_policy,
                        expiry_deadline_millis: record.expiry_deadline_millis,
                    })
                }
                Some(current) => Some(current.clone()),
            },
            StreamSessionRecord::ExpiryRefreshed(record) => match current {
                Some(Self::Live {
                    session_key,
                    expiry_policy,
                    expiry_deadline_millis: Some(deadline),
                }) if session_key == &record.session_key
                    && *deadline == record.expected_deadline_millis
                    && matches!(
                        expiry_policy,
                        crate::model::durable_stream::StreamSessionExpiryPolicy::Sliding { .. }
                    ) =>
                {
                    Some(Self::Live {
                        session_key: record.session_key.clone(),
                        expiry_policy: *expiry_policy,
                        expiry_deadline_millis: Some(record.deadline_millis),
                    })
                }
                _ => current.cloned(),
            },
            StreamSessionRecord::Expired(record) => match current {
                Some(Self::Live {
                    session_key,
                    expiry_deadline_millis: Some(deadline),
                    ..
                }) if session_key == &record.session_key
                    && *deadline == record.expected_deadline_millis =>
                {
                    Some(Self::Retired {
                        session_key: record.session_key.clone(),
                    })
                }
                _ => current.cloned(),
            },
            _ => current.cloned(),
        }
    }
}

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
        record: &crate::model::durable_stream::StreamSessionRecord,
    ) {
        use crate::model::durable_stream::StreamSessionRecord;

        if matches!(record, StreamSessionRecord::ForkCut(_)) {
            let retained: Vec<_> = self
                .iter()
                .map(|(key, status)| (key, status.clone()))
                .collect();
            for (key, mut status) in retained {
                status.apply_record(index, record);
                self.insert(key, status);
            }
            return;
        }

        let Some(key) = record.local_session_key() else {
            return;
        };
        let mut status = match self.get(key) {
            Some(status) => status.clone(),
            None if matches!(
                record,
                StreamSessionRecord::Prepared(_) | StreamSessionRecord::ExportForkInitialized(_)
            ) =>
            {
                Default::default()
            }
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
    pub pending_update: Option<PendingUpdateRef>,
    pub snapshot_assisted_details: Option<oplog::FailedSnapshotAssistedUpdateDetails>,
    /// Whether the failure is about the record that the update selected. `None` for any other
    /// failure.
    pub snapshot_fault: Option<oplog::SnapshotFault>,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct SuccessfulUpdateRecord {
    pub timestamp: Timestamp,
    pub target_revision: ComponentRevision,
    /// The index of the `SuccessfulUpdate` entry. It orders the update against other entries,
    /// which the timestamps of different executors cannot do.
    pub oplog_index: OplogIndex,
    /// The filesystem snapshot of the applied snapshot-based update. `None` for an automatic
    /// update, for an update without a filesystem capture, and when no pending update was in
    /// front of the queue.
    pub filesystem_snapshot: Option<FilesystemSnapshotName>,
    pub pending_update: Option<PendingUpdateRef>,
    pub snapshot_assisted_details: Option<oplog::SnapshotAssistedUpdateDetails>,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct AuthoritativeSnapshot {
    /// The `PendingUpdate` entry of a manual update, or the `Snapshot` entry of the record that a
    /// snapshot-assisted automatic update selected.
    pub index: OplogIndex,
    pub kind: AuthoritativeSnapshotKind,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum AuthoritativeSnapshotKind {
    ManualUpdate,
    SnapshotAssistedAutomatic {
        /// The filesystem snapshot of the selected record, or `None` when it has no name.
        filesystem_snapshot: Option<FilesystemSnapshotName>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct OplogProcessorCheckpointState {
    pub target_agent_id: Option<AgentId>,
    pub confirmed_up_to: OplogIndex,
    pub sending_up_to: OplogIndex,
    pub last_batch_start: OplogIndex,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct PendingCardEventRef {
    pub timestamp: Timestamp,
    pub oplog_index: OplogIndex,
    pub entity_parent_start_index: Option<OplogIndex>,
    pub event: QueuedCardEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum PendingUpdateKind {
    Automatic,
    SnapshotAssistedAutomatic(Box<AssistedSelection>),
    SnapshotBased {
        /// The filesystem snapshot that the executor captured with the application snapshot.
        /// `None` means that the executor made no filesystem capture.
        filesystem_snapshot: Option<FilesystemSnapshotName>,
    },
}

/// The record that a snapshot-assisted automatic update selected, and the source that it was
/// selected from.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct AssistedSelection {
    /// The index of the entry that started the source revision (`Create` or the last
    /// `SuccessfulUpdate`) when the record was selected.
    pub source_revision_start_index: OplogIndex,
    /// The selected record. Its `component_revision` is the source revision.
    pub snapshot: UsableAutomaticSnapshot,
}

impl PendingUpdateKind {
    /// The filesystem snapshot of the update, when it has one: the capture of a snapshot-based
    /// update, or the filesystem snapshot of the record that a snapshot-assisted automatic update
    /// selected.
    pub fn filesystem_snapshot(&self) -> Option<&FilesystemSnapshotName> {
        match self {
            Self::Automatic => None,
            Self::SnapshotAssistedAutomatic(selection) => {
                selection.snapshot.filesystem_snapshot.as_ref()
            }
            Self::SnapshotBased {
                filesystem_snapshot,
            } => filesystem_snapshot.as_ref(),
        }
    }

    /// The kind of the pending update that `description` describes.
    pub fn of(description: &oplog::UpdateDescription) -> Self {
        match description {
            oplog::UpdateDescription::Automatic { .. } => Self::Automatic,
            oplog::UpdateDescription::SnapshotAssistedAutomatic {
                source_revision_start_index,
                snapshot_index,
                snapshot_revision,
                filesystem_snapshot,
                ..
            } => Self::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
                source_revision_start_index: *source_revision_start_index,
                snapshot: UsableAutomaticSnapshot {
                    index: *snapshot_index,
                    component_revision: *snapshot_revision,
                    filesystem_snapshot: filesystem_snapshot.clone(),
                },
            })),
            oplog::UpdateDescription::SnapshotBased {
                filesystem_snapshot,
                ..
            } => Self::SnapshotBased {
                filesystem_snapshot: filesystem_snapshot.clone(),
            },
        }
    }
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
    /// Durable admission identity returned to the caller. For manual updates this is the
    /// originating `PendingAgentInvocation` index. For automatic updates it identifies the first
    /// `PendingUpdate`, while `oplog_index` can identify the later strategy-selection entry.
    pub admission_index: OplogIndex,
    pub target_revision: ComponentRevision,
    pub kind: PendingUpdateKind,
}
