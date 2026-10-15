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

use crate::durable_host::replay_state::ReplayEvent;
use crate::durable_host::revision_update::{
    AgentInputs, RevisionUpdate, RevisionUpdateInputs, prepare_revision_update,
};
use crate::durable_host::{
    add_wallet_card, adopt_recorded_wallet_generation, agent_initial_card_from_component_metadata,
    apply_invocation_wallet_pin, card_holder_is_agent, remove_wallet_card, remove_wallet_cards,
    replace_wallet_cards, transfer_started_removes_source_membership,
};
use crate::services::card::CardState;
use crate::services::card_interest::{
    CardInterestContribution, CardInterestGeneration, CardInterestIndex,
};
use crate::services::component::ComponentService;
use crate::services::oplog::Oplog;
use crate::services::{HasCardService, HasOplog, HasWorkerProxy};
use crate::worker::Worker;
use crate::worker::tasks::{RuntimeAppendTasks, WorkerTasks};
use crate::workerctx::WorkerCtx;
use arc_swap::ArcSwap;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::model::agent::ResolvedOwnerContext;
use golem_common::model::card::{AgentCardHolder, CardHolder, CardId, ScopeCard, StoredCard};
use golem_common::model::oplog::{CardInstallFailure, OplogEntry, OplogIndex, QueuedCardEvent};
use golem_common::model::worker::TypedAgentConfigEntry;
use golem_common::model::{OwnedAgentId, Timestamp};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::component::Component;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::OwnedMutexGuard;
use tokio_util::task::task_tracker::TaskTrackerToken;

pub(crate) enum CardEventBoundary {
    Historical,
    Live {
        owner: Arc<OwnerWallet>,
        _guard: OwnedMutexGuard<()>,
        _append_admission: TaskTrackerToken,
    },
}

#[derive(Clone)]
pub(crate) struct AuthorityScopeInputs {
    pub(crate) scope: Option<ScopeCard>,
    pub(crate) roots: Vec<StoredCard>,
    pub(crate) unchecked_root_ids: Option<Vec<CardId>>,
    pub(crate) parent: Option<OplogIndex>,
}

pub(crate) struct OwnerAuthoritySnapshot {
    pub(crate) wallet: Arc<WalletSnapshot>,
    pub(crate) generation: u64,
    pub(crate) scope: Option<ScopeCard>,
    pub(crate) revoked: HashSet<CardId>,
    pub(crate) root_states: Option<HashMap<CardId, CardState>>,
}

/// Accepted publication owns its boundary and append admission until commit and invalidation.
pub(crate) struct WalletCommit {
    owner: Arc<OwnerWallet>,
    entry: OplogEntry,
    predecessor: Arc<WalletSnapshot>,
    next: Arc<WalletSnapshot>,
    _boundary: Arc<CardEventBoundary>,
    _candidate: Option<Arc<CardInterestContribution>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WalletPublicationFailure {
    RetiredGeneration,
    PredecessorChanged,
}

impl WalletCommit {
    pub(crate) async fn prepare(
        boundary: Arc<CardEventBoundary>,
        mut entry: OplogEntry,
    ) -> Result<Self, WorkerExecutorError> {
        let CardEventBoundary::Live { owner, .. } = boundary.as_ref() else {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "live owner wallet transaction",
                "historical wallet boundary",
            ));
        };
        owner.validate_current()?;
        let predecessor = owner.snapshot();
        let mut next = predecessor.as_ref().clone();
        next.apply(
            &owner.baseline.owner_id,
            &owner.baseline.owner_context,
            owner.baseline.component_service.as_ref(),
            &mut entry,
            false,
            false,
        )
        .await?;
        Ok(Self {
            owner: owner.clone(),
            entry,
            predecessor,
            next: Arc::new(next),
            _boundary: boundary,
            _candidate: None,
        })
    }

    pub(crate) fn entry(&self) -> OplogEntry {
        self.entry.clone()
    }

    pub(crate) fn validate_before_append(&self) -> Result<(), WorkerExecutorError> {
        self.owner.validate_current()?;
        if Arc::ptr_eq(&self.owner.snapshot(), &self.predecessor) {
            Ok(())
        } else {
            Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Jump,
            })
        }
    }

    pub(crate) fn publish(&self) -> Result<(), WalletPublicationFailure> {
        self.owner.publish(&self.predecessor, self.next.clone())
    }
}

/// Membership and its durable version always travel as one immutable publication.
#[derive(Clone, Debug)]
pub(crate) struct WalletSnapshot {
    pub(crate) cards: BTreeMap<CardId, StoredCard>,
    pub(crate) generation: u64,
    pub(crate) wallet_id_hash: [u8; 32],
    pub(crate) metadata: Arc<Component>,
}

pub(crate) struct WalletReplayProjection {
    pub(crate) wallet: WalletSnapshot,
    pub(crate) update: Option<Arc<RevisionUpdate>>,
}

struct ReplayDerivation {
    card: StoredCard,
    receipt: WalletReplayReceipt,
}

struct ReplayDerivationEntry {
    parent: Option<OplogIndex>,
    evidence: Option<ReplayDerivation>,
}

#[derive(Clone)]
pub struct WalletReplayReceipt {
    pub(crate) index: OplogIndex,
    pub(crate) result:
        Shared<BoxFuture<'static, Result<Arc<WalletReplayProjection>, WorkerExecutorError>>>,
    identity: Arc<()>,
}

impl std::fmt::Debug for WalletReplayReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletReplayReceipt")
            .field("index", &self.index)
            .field("identity", &Arc::as_ptr(&self.identity))
            .finish()
    }
}

impl PartialEq for WalletReplayReceipt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl Eq for WalletReplayReceipt {}

/// An outstanding entity's committed prefix, frozen at its first physical replay boundary.
/// Before that boundary, the claim-time prefix lets pre-Start body work run without waiting.
pub(crate) struct EntityReplayWallet {
    prefix: std::sync::Mutex<(WalletReplayReceipt, bool)>,
}

impl EntityReplayWallet {
    pub(crate) fn receipt(&self) -> WalletReplayReceipt {
        self.prefix.lock().unwrap().0.clone()
    }
}

type ReplayProjectionResult = Result<Arc<WalletReplayProjection>, WorkerExecutorError>;

/// Even refusal, panic and task cancellation resolve accepted receipts and retire the owner.
struct ReplayProjectionCompletion {
    owner: Arc<OwnerWallet>,
    sender: Option<tokio::sync::oneshot::Sender<ReplayProjectionResult>>,
}

impl ReplayProjectionCompletion {
    fn complete(mut self, result: ReplayProjectionResult) {
        if result.is_err() {
            self.owner.retire();
        }
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(result);
        }
    }
}

impl Drop for ReplayProjectionCompletion {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            self.owner.retire();
            let _ = sender.send(Err(WorkerExecutorError::runtime(
                "accepted physical wallet projection did not complete",
            )));
        }
    }
}

impl WalletSnapshot {
    /// Store delivery changes membership independently of the physical version already observed.
    pub(crate) fn apply_delivered_membership(
        &mut self,
        agent_id: &golem_common::model::AgentId,
        event: &ReplayEvent,
    ) -> Result<(), WorkerExecutorError> {
        match event {
            ReplayEvent::CardTransferred { target_holder, .. }
                if !card_holder_is_agent(target_holder, agent_id) => {}
            ReplayEvent::CardInstalled { card, .. } | ReplayEvent::CardTransferred { card, .. } => {
                if let Some(existing) = self.cards.get(&card.card_id()) {
                    if existing != card {
                        return Err(WorkerExecutorError::runtime(format!(
                            "wallet card {} conflicts with its existing payload",
                            card.card_id()
                        )));
                    }
                } else {
                    self.cards.insert(card.card_id(), card.clone());
                }
            }
            ReplayEvent::CardTransferStarted {
                card_id,
                source_holder,
                ..
            } => {
                if transfer_started_removes_source_membership(
                    self.cards.get(card_id),
                    source_holder,
                    agent_id,
                ) {
                    self.cards.remove(card_id);
                }
            }
            ReplayEvent::CardRevokedCascade { card_ids, .. } => {
                for card_id in card_ids {
                    self.cards.remove(card_id);
                }
            }
            ReplayEvent::CardRevoked { card_id, .. } | ReplayEvent::CardExpired { card_id, .. } => {
                self.cards.remove(card_id);
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn apply_replay_event(
        &mut self,
        agent_id: &golem_common::model::AgentId,
        event: &crate::durable_host::replay_state::ReplayEvent,
    ) -> Result<(), WorkerExecutorError> {
        use crate::durable_host::replay_state::ReplayEvent;
        let recorded = match event {
            ReplayEvent::InvocationWalletPinned { wallet_pin, .. } => {
                apply_invocation_wallet_pin(
                    &mut self.cards,
                    self.wallet_id_hash,
                    &mut self.generation,
                    wallet_pin.clone(),
                )?;
                None
            }
            ReplayEvent::CardInstalled {
                card,
                wallet_generation,
            } => {
                add_wallet_card(&mut self.cards, &mut self.generation, card.clone())?;
                Some(*wallet_generation)
            }
            ReplayEvent::CardDerived {
                wallet_generation, ..
            } => Some(*wallet_generation),
            ReplayEvent::CardTransferStarted {
                card_id,
                source_holder,
                source_wallet_generation,
                ..
            } if card_holder_is_agent(source_holder, agent_id) => {
                if transfer_started_removes_source_membership(
                    self.cards.get(card_id),
                    source_holder,
                    agent_id,
                ) {
                    remove_wallet_card(&mut self.cards, &mut self.generation, *card_id)?;
                }
                Some(*source_wallet_generation)
            }
            ReplayEvent::CardTransferred {
                target_holder,
                card,
                target_wallet_generation,
                ..
            } if card_holder_is_agent(target_holder, agent_id) => {
                add_wallet_card(&mut self.cards, &mut self.generation, card.clone())?;
                Some(*target_wallet_generation)
            }
            ReplayEvent::CardRevokedCascade {
                card_ids,
                local_wallet_generation,
            } => {
                remove_wallet_cards(&mut self.cards, &mut self.generation, card_ids)?;
                Some(*local_wallet_generation)
            }
            ReplayEvent::CardRevoked {
                card_id,
                wallet_generation,
            }
            | ReplayEvent::CardExpired {
                card_id,
                wallet_generation,
            } => {
                remove_wallet_card(&mut self.cards, &mut self.generation, *card_id)?;
                Some(*wallet_generation)
            }
            _ => None,
        };
        if let Some(generation) = recorded {
            adopt_recorded_wallet_generation(&mut self.generation, generation)?;
        }
        Ok(())
    }

    pub(crate) fn initial(
        owner_id: &OwnedAgentId,
        owner_context: &ResolvedOwnerContext,
        metadata: &Component,
    ) -> Result<Self, WorkerExecutorError> {
        let card = initial_card(owner_context, metadata)?;
        Ok(Self {
            cards: BTreeMap::from([(card.card_id(), card)]),
            generation: 0,
            metadata: Arc::new(metadata.clone()),
            wallet_id_hash: CardHolder::Agent(AgentCardHolder {
                agent_id: owner_id.agent_id.clone(),
            })
            .wallet_id_hash(),
        })
    }

    async fn apply(
        &mut self,
        owner_id: &OwnedAgentId,
        owner_context: &ResolvedOwnerContext,
        component_service: &dyn ComponentService,
        entry: &mut OplogEntry,
        skipped: bool,
        recorded: bool,
    ) -> Result<(), WorkerExecutorError> {
        match entry {
            OplogEntry::CardInstalled {
                card,
                wallet_generation,
                ..
            } => {
                add_wallet_card(&mut self.cards, &mut self.generation, card.as_ref().clone())?;
                self.apply_generation(wallet_generation, recorded)?;
            }
            OplogEntry::CardDerived {
                wallet_generation, ..
            } => {
                if !recorded {
                    *wallet_generation = self.generation;
                } else if *wallet_generation != self.generation {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "CardDerived generation from the preceding wallet prefix",
                        format!(
                            "audit generation {wallet_generation} differs from preceding generation {}",
                            self.generation
                        ),
                    ));
                }
            }
            OplogEntry::CardTransferStarted {
                card_id,
                source_holder,
                source_wallet_generation,
                ..
            } if card_holder_is_agent(source_holder, &owner_id.agent_id) => {
                if transfer_started_removes_source_membership(
                    self.cards.get(card_id),
                    source_holder,
                    &owner_id.agent_id,
                ) {
                    remove_wallet_card(&mut self.cards, &mut self.generation, *card_id)?;
                }
                self.apply_generation(source_wallet_generation, recorded)?;
            }
            OplogEntry::CardTransferred {
                target_holder,
                card,
                target_wallet_generation,
                ..
            } if card_holder_is_agent(target_holder, &owner_id.agent_id) => {
                add_wallet_card(&mut self.cards, &mut self.generation, card.as_ref().clone())?;
                self.apply_generation(target_wallet_generation, recorded)?;
            }
            OplogEntry::CardRevoked {
                card_id,
                wallet_generation,
                ..
            }
            | OplogEntry::CardExpired {
                card_id,
                wallet_generation,
                ..
            } => {
                remove_wallet_card(&mut self.cards, &mut self.generation, *card_id)?;
                self.apply_generation(wallet_generation, recorded)?;
            }
            OplogEntry::CardRevokedCascade {
                revoked_card_ids,
                local_wallet_generation,
                affected_wallets,
                ..
            } => {
                let changed =
                    remove_wallet_cards(&mut self.cards, &mut self.generation, revoked_card_ids)?;
                if !recorded {
                    *affected_wallets = if changed {
                        vec![CardHolder::Agent(AgentCardHolder {
                            agent_id: owner_id.agent_id.clone(),
                        })]
                    } else {
                        Vec::new()
                    };
                }
                self.apply_generation(local_wallet_generation, recorded)?;
            }
            OplogEntry::AgentInvocationStarted { wallet_pin, .. } if !skipped => {
                apply_invocation_wallet_pin(
                    &mut self.cards,
                    self.wallet_id_hash,
                    &mut self.generation,
                    wallet_pin.as_ref().clone(),
                )?;
            }
            OplogEntry::SuccessfulUpdate {
                target_revision, ..
            } if !skipped => {
                let metadata = component_service
                    .get_metadata(owner_id.component_id(), Some(*target_revision))
                    .await?;
                if owner_context.agent().is_some() {
                    let card = initial_card(owner_context, &metadata)?;
                    replace_wallet_cards(
                        &mut self.cards,
                        &mut self.generation,
                        BTreeMap::from([(card.card_id(), card)]),
                    )?;
                }
                self.metadata = Arc::new(metadata);
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_generation(
        &mut self,
        entry_generation: &mut u64,
        recorded: bool,
    ) -> Result<(), WorkerExecutorError> {
        if recorded {
            adopt_recorded_wallet_generation(&mut self.generation, *entry_generation)
        } else {
            *entry_generation = self.generation;
            Ok(())
        }
    }
}

fn initial_card(
    owner_context: &ResolvedOwnerContext,
    metadata: &Component,
) -> Result<StoredCard, WorkerExecutorError> {
    match owner_context {
        ResolvedOwnerContext::Agent(agent_id) => {
            agent_initial_card_from_component_metadata(metadata, agent_id)
        }
        ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
            Ok(StoredCard::Polymorphic(
                metadata
                    .metadata
                    .component_provision_config()
                    .initial_permissions
                    .clone(),
            ))
        }
    }
}

/// The selected snapshot or initial wallet from which ordinary replay starts.
#[derive(Clone)]
pub(crate) struct WalletBaseline {
    owner_id: OwnedAgentId,
    owner_context: ResolvedOwnerContext,
    component_service: Arc<dyn ComponentService>,
    metadata: Arc<Component>,
    snapshot: WalletSnapshot,
    through: OplogIndex,
    initial_agent_config: Vec<TypedAgentConfigEntry>,
}

impl WalletBaseline {
    pub(crate) fn owner_context(&self) -> &ResolvedOwnerContext {
        &self.owner_context
    }

    pub(crate) async fn new(
        owner_id: OwnedAgentId,
        owner_context: ResolvedOwnerContext,
        component_service: Arc<dyn ComponentService>,
        metadata: &Component,
        oplog: &dyn Oplog,
        snapshot_index: Option<OplogIndex>,
        initial_agent_config: Vec<TypedAgentConfigEntry>,
    ) -> Result<Self, WorkerExecutorError> {
        let (snapshot, through) = match snapshot_index {
            Some(index) => match oplog.read(index).await {
                OplogEntry::Snapshot {
                    active_cards,
                    wallet_generation,
                    ..
                } => (
                    WalletSnapshot {
                        cards: active_cards
                            .into_iter()
                            .map(|card| (card.card_id(), card))
                            .collect(),
                        generation: wallet_generation,
                        metadata: Arc::new(metadata.clone()),
                        wallet_id_hash: CardHolder::Agent(AgentCardHolder {
                            agent_id: owner_id.agent_id.clone(),
                        })
                        .wallet_id_hash(),
                    },
                    index,
                ),
                _ => (
                    WalletSnapshot::initial(&owner_id, &owner_context, metadata)?,
                    OplogIndex::NONE,
                ),
            },
            None => (
                WalletSnapshot::initial(&owner_id, &owner_context, metadata)?,
                OplogIndex::NONE,
            ),
        };
        Ok(Self {
            owner_id,
            owner_context,
            component_service,
            metadata: Arc::new(metadata.clone()),
            snapshot,
            through,
            initial_agent_config,
        })
    }

    pub(crate) async fn reselect(
        &self,
        oplog: &dyn Oplog,
        snapshot_index: Option<OplogIndex>,
    ) -> Result<Self, WorkerExecutorError> {
        Self::new(
            self.owner_id.clone(),
            self.owner_context.clone(),
            self.component_service.clone(),
            &self.metadata,
            oplog,
            snapshot_index,
            self.initial_agent_config.clone(),
        )
        .await
    }
}

/// The only live wallet writer for one resident replay generation.
pub(crate) struct OwnerWallet {
    snapshot: ArcSwap<WalletSnapshot>,
    latest_replay: std::sync::Mutex<WalletReplayReceipt>,
    /// Receipts and consumed IDs live until their primary invocation or entity body ends.
    derivations: std::sync::Mutex<HashMap<CardId, ReplayDerivationEntry>>,
    entity_prefixes: std::sync::Mutex<HashMap<OplogIndex, Arc<EntityReplayWallet>>>,
    replay_published: AtomicBool,
    replay_complete: AtomicBool,
    replay_progress: tokio::sync::Notify,
    pub(crate) tasks: WorkerTasks,
    active: AtomicBool,
    pub(crate) replay_preparations:
        std::sync::Mutex<crate::durable_host::replay_application::ReplayPreparationOrder>,
    card_boundary: Arc<tokio::sync::Mutex<()>>,
    pub(crate) baseline: WalletBaseline,
    pub(crate) append_tasks: Arc<RuntimeAppendTasks>,
    pub(crate) interest: Arc<CardInterestGeneration>,
}

impl std::fmt::Debug for OwnerWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerWallet")
            .field("replay_index", &self.latest_replay_receipt().index)
            .finish_non_exhaustive()
    }
}

impl OwnerWallet {
    pub(crate) async fn new(
        baseline: WalletBaseline,
        append_tasks: Arc<RuntimeAppendTasks>,
        tasks: WorkerTasks,
        card_boundary: Arc<tokio::sync::Mutex<()>>,
        card_interest_index: Arc<CardInterestIndex>,
    ) -> Result<Arc<Self>, WorkerExecutorError> {
        let snapshot = baseline.snapshot.clone();
        let projection = Arc::new(WalletReplayProjection {
            wallet: snapshot.clone(),
            update: None,
        });
        let latest_replay = WalletReplayReceipt {
            index: baseline.through,
            result: futures::future::ready(Ok(projection)).boxed().shared(),
            identity: Arc::new(()),
        };
        let interest = card_interest_index.generation(baseline.owner_id.clone());
        Ok(Arc::new(Self {
            snapshot: ArcSwap::from_pointee(snapshot),
            latest_replay: std::sync::Mutex::new(latest_replay),
            derivations: Default::default(),
            entity_prefixes: Default::default(),
            replay_published: AtomicBool::new(false),
            replay_complete: AtomicBool::new(false),
            replay_progress: tokio::sync::Notify::new(),
            tasks,
            active: AtomicBool::new(true),
            replay_preparations: Default::default(),
            card_boundary,
            baseline,
            append_tasks,
            interest,
        }))
    }

    pub(crate) async fn activate_interest(&self) {
        self.interest
            .activate(&self.snapshot().cards.keys().copied().collect::<Vec<_>>())
            .await;
    }

    pub(crate) fn latest_replay_receipt(&self) -> WalletReplayReceipt {
        self.latest_replay.lock().unwrap().clone()
    }

    pub(crate) fn register_replayed_entity(&self, start: OplogIndex) {
        self.entity_prefixes
            .lock()
            .unwrap()
            .entry(start)
            .or_insert_with(|| {
                Arc::new(EntityReplayWallet {
                    prefix: std::sync::Mutex::new((self.latest_replay_receipt(), false)),
                })
            });
    }

    pub(crate) fn capture_entity_prefix(&self, start: OplogIndex) {
        if let Some(entity) = self.entity_prefixes.lock().unwrap().get(&start) {
            let mut prefix = entity.prefix.lock().unwrap();
            if !prefix.1 {
                *prefix = (self.latest_replay_receipt(), true);
            }
        }
    }

    pub(crate) fn entity_replay_wallet(
        &self,
        start: OplogIndex,
    ) -> Result<Arc<EntityReplayWallet>, WorkerExecutorError> {
        self.entity_prefixes
            .lock()
            .unwrap()
            .get(&start)
            .cloned()
            .ok_or_else(|| {
                WorkerExecutorError::runtime("historical entity has no claimed wallet prefix")
            })
    }

    pub(crate) fn prune_entity_prefixes(&self, mut deleted: impl FnMut(OplogIndex) -> bool) {
        self.entity_prefixes
            .lock()
            .unwrap()
            .retain(|start, _| !deleted(*start));
    }

    pub(crate) fn finish_replay_prefix(&self) {
        let _latest = self.latest_replay.lock().unwrap();
        self.replay_complete.store(true, Ordering::Release);
        self.replay_progress.notify_waiters();
    }

    pub(crate) fn reopen_replay_prefix(&self) -> Result<(), WorkerExecutorError> {
        let _latest = self.latest_replay.lock().unwrap();
        if self.replay_published.load(Ordering::Acquire) {
            self.retire();
            return Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Jump,
            });
        }
        self.replay_complete.store(false, Ordering::Release);
        Ok(())
    }

    /// Reserves committed physical order, independently of Store delivery and card boundaries.
    pub(crate) fn reserve_replay_event(
        self: &Arc<Self>,
        index: OplogIndex,
        event: &ReplayEvent,
    ) -> WalletReplayReceipt {
        let mut latest = self.latest_replay.lock().unwrap();
        if index <= self.baseline.through {
            return WalletReplayReceipt {
                index: self.baseline.through,
                result: futures::future::ready(Ok(Arc::new(WalletReplayProjection {
                    wallet: self.baseline.snapshot.clone(),
                    update: None,
                })))
                .boxed()
                .shared(),
                identity: Arc::new(()),
            };
        }
        if index == latest.index {
            return latest.clone();
        }
        let predecessor = latest.clone();
        let admission = self.append_tasks.register();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let receipt = WalletReplayReceipt {
            index,
            result: async move {
                receiver.await.unwrap_or_else(|_| {
                    Err(WorkerExecutorError::runtime(
                        "physical wallet projection lost its result",
                    ))
                })
            }
            .boxed()
            .shared(),
            identity: Arc::new(()),
        };
        let completion = ReplayProjectionCompletion {
            owner: self.clone(),
            sender: Some(sender),
        };
        if index < predecessor.index {
            completion.complete(Err(WorkerExecutorError::unexpected_oplog_entry(
                "strictly increasing physical wallet reservation",
                format!("index {index} follows {}", predecessor.index),
            )));
            return receipt;
        }
        *latest = receipt.clone();
        let owner = self.clone();
        let event = event.clone();
        // The completion guard is captured before spawning, so refusal also resolves the receipt.
        let _ = self.tasks.spawn_metadata(async move {
            let result = async {
                let _admission = admission.ok_or(WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Jump,
                })?;
                let previous = predecessor.result.await?;
                owner.validate_current()?;
                let mut wallet = previous.wallet.clone();
                drop(previous);
                let update = match &event {
                    ReplayEvent::UpdateReplayed { new_revision }
                        if *new_revision > wallet.metadata.revision =>
                    {
                        let update = prepare_revision_update(
                            RevisionUpdateInputs {
                                component_service: owner.baseline.component_service.clone(),
                                component_id: owner.baseline.owner_id.component_id(),
                            },
                            *new_revision,
                            || AgentInputs {
                                agent_id: owner.baseline.owner_context.agent().cloned(),
                                initial_agent_config: owner.baseline.initial_agent_config.clone(),
                            },
                        )
                        .await
                        .map_err(|error| error.to_worker_executor_error())?;
                        update.project_wallet(&mut wallet)?;
                        Some(Arc::new(update))
                    }
                    ReplayEvent::CardDerived {
                        wallet_generation, ..
                    } => {
                        if *wallet_generation != wallet.generation {
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                "CardDerived generation from its physical predecessor",
                                format!(
                                    "audit {wallet_generation}, predecessor {}",
                                    wallet.generation
                                ),
                            ));
                        }
                        None
                    }
                    _ => {
                        wallet.apply_replay_event(&owner.baseline.owner_id.agent_id, &event)?;
                        None
                    }
                };
                Ok(Arc::new(WalletReplayProjection { wallet, update }))
            }
            .await;
            completion.complete(result);
        });
        receipt
    }

    pub(crate) fn lock_boundary(
        self: &Arc<Self>,
    ) -> impl std::future::Future<Output = Result<Arc<CardEventBoundary>, WorkerExecutorError>>
    + Send
    + 'static
    + use<> {
        let owner = self.clone();
        let append_admission = self.append_tasks.register();
        async move {
            let append_admission = append_admission.ok_or(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Jump,
            })?;
            loop {
                let progress = owner.replay_progress.notified();
                tokio::pin!(progress);
                progress.as_mut().enable();
                owner.validate_current()?;
                if !owner.replay_complete.load(Ordering::Acquire) {
                    progress.await;
                    continue;
                }
                let receipt = owner.latest_replay_receipt();
                let replay = receipt.result.clone().await?;
                let guard = owner.card_boundary.clone().lock_owned().await;
                {
                    let latest = owner.latest_replay.lock().unwrap();
                    owner.validate_current()?;
                    if !owner.replay_complete.load(Ordering::Acquire) || *latest != receipt {
                        continue;
                    }
                    if !owner.replay_published.load(Ordering::Acquire) {
                        owner
                            .publish(&owner.snapshot(), Arc::new(replay.wallet.clone()))
                            .map_err(|failure| {
                                WorkerExecutorError::runtime(format!(
                                    "failed to publish reconstructed wallet: {failure:?}"
                                ))
                            })?;
                        owner.replay_published.store(true, Ordering::Release);
                    }
                }
                return Ok(Arc::new(CardEventBoundary::Live {
                    owner: owner.clone(),
                    _guard: guard,
                    _append_admission: append_admission,
                }));
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<WalletSnapshot> {
        self.snapshot.load_full()
    }

    pub(crate) fn validate_current(&self) -> Result<(), WorkerExecutorError> {
        if self.active.load(Ordering::Acquire) && self.interest.is_current() {
            Ok(())
        } else {
            Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Jump,
            })
        }
    }

    pub(crate) fn retire(&self) {
        self.active.store(false, Ordering::Release);
        self.interest.retire();
        self.replay_progress.notify_waiters();
    }

    pub(crate) fn publish(
        &self,
        predecessor: &Arc<WalletSnapshot>,
        snapshot: Arc<WalletSnapshot>,
    ) -> Result<(), WalletPublicationFailure> {
        let failure = if self.validate_current().is_err() {
            Some(WalletPublicationFailure::RetiredGeneration)
        } else if !Arc::ptr_eq(&self.snapshot.load_full(), predecessor) {
            Some(WalletPublicationFailure::PredecessorChanged)
        } else {
            None
        };
        if let Some(failure) = failure {
            self.retire();
            return Err(failure);
        }
        self.interest.set_wallet(snapshot.cards.keys().copied());
        self.snapshot.store(snapshot);
        Ok(())
    }

    pub(crate) fn initial_historical_snapshot(&self) -> WalletSnapshot {
        self.baseline.snapshot.clone()
    }

    pub(crate) fn pending_replay_finalization(
        &self,
    ) -> Option<Arc<crate::durable_host::replay_application::ReplayApplication>> {
        let order = self.replay_preparations.lock().unwrap();
        order
            .finalization
            .upgrade()
            .filter(|application| !matches!(application.result.peek(), Some(Ok(_))))
    }

    async fn join_replay_finalization(&self) -> Result<(), WorkerExecutorError> {
        while let Some(application) = self.pending_replay_finalization() {
            application.result.clone().await?;
        }
        Ok(())
    }

    async fn validate_boundary<Ctx: WorkerCtx>(
        self: &Arc<Self>,
        worker: &Worker<Ctx>,
        boundary: &CardEventBoundary,
    ) -> Result<(), WorkerExecutorError> {
        self.validate_current()?;
        let CardEventBoundary::Live { owner, .. } = boundary else {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "live owner wallet boundary",
                "historical wallet boundary",
            ));
        };
        if !Arc::ptr_eq(self, owner)
            || !Arc::ptr_eq(self, &worker.owner_execution().wallet().await?)
        {
            return Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Jump,
            });
        }
        Ok(())
    }

    pub(crate) async fn commit<Ctx: WorkerCtx>(
        self: &Arc<Self>,
        worker: &Arc<Worker<Ctx>>,
        boundary: Arc<CardEventBoundary>,
        entry: OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        self.validate_boundary(worker, boundary.as_ref()).await?;
        let prepared = WalletCommit::prepare(boundary, entry).await?;
        worker
            .state_actor
            .append_and_commit_wallet(worker.clone(), prepared)
            .await
    }

    pub(crate) fn synchronize<Ctx: WorkerCtx>(
        self: &Arc<Self>,
        worker: Arc<Worker<Ctx>>,
        inputs: AuthorityScopeInputs,
        requires_authority: bool,
        wait_for_authority: bool,
    ) -> impl std::future::Future<
        Output = Result<Option<OwnerAuthoritySnapshot>, WorkerExecutorError>,
    >
    + Send
    + 'static
    + use<Ctx> {
        let owner = self.clone();
        let admission = self.append_tasks.register();
        async move {
            let _admission = admission.ok_or(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Jump,
            })?;
            loop {
                owner.join_replay_finalization().await?;
                let boundary = owner.lock_boundary().await?;
                if let Some(application) = owner.pending_replay_finalization() {
                    drop(boundary);
                    application.result.clone().await?;
                    continue;
                }
                if requires_authority && !worker.card_interest_index.authority_is_open() {
                    drop(boundary);
                    if !wait_for_authority {
                        return Ok(None);
                    }
                    worker.card_interest_index.wait_until_authority_open().await;
                    continue;
                }
                owner
                    .synchronize_locked(&worker, boundary.clone(), inputs.parent)
                    .await?;
                let expired_roots = inputs
                    .roots
                    .iter()
                    .filter_map(|card| {
                        card.expires_at()
                            .filter(|at| *at <= chrono::Utc::now())
                            .map(|_| card.card_id())
                    })
                    .collect::<Vec<_>>();
                if !expired_roots.is_empty() {
                    owner
                        .commit(
                            &worker,
                            boundary.clone(),
                            OplogEntry::CardRevokedCascade {
                                timestamp: Timestamp::now_utc(),
                                entity_parent_start_index: inputs.parent,
                                revoked_card_ids: expired_roots,
                                affected_wallets: Vec::new(),
                                local_wallet_generation: 0,
                            },
                        )
                        .await?;
                }
                let retries = owner
                    .prepare_source_transfers(&worker, boundary.clone())
                    .await?;
                if !retries.is_empty() {
                    drop(boundary);
                    for retry in retries {
                        worker
                            .worker_proxy()
                            .deliver_card_transfer(
                                &retry.target_agent_id,
                                worker.owned_agent_id.environment_id,
                                retry.transfer_id,
                                retry.source_card_id,
                                &retry.installed_card,
                            )
                            .await
                            .map_err(|error| {
                                WorkerExecutorError::runtime(format!(
                                    "permission-card transfer delivery failed: {error}"
                                ))
                            })?;
                        let confirmation = owner.lock_boundary().await?;
                        if !retry
                            .is_confirmed(worker.oplog().as_ref(), &worker.owned_agent_id.agent_id)
                            .await?
                        {
                            owner
                                .commit(
                                    &worker,
                                    confirmation.clone(),
                                    OplogEntry::card_transfer_confirmed(
                                        retry.entity_parent_start_index(),
                                        retry.transfer_id,
                                        retry.source_card_id,
                                        retry.installed_card.card_id(),
                                        CardHolder::Agent(AgentCardHolder {
                                            agent_id: retry.target_agent_id.clone(),
                                        }),
                                    ),
                                )
                                .await?;
                        }
                    }
                    continue;
                }
                let root_states = match &inputs.unchecked_root_ids {
                    Some(ids) => Some(worker.card_service().check_cards(ids.clone()).await?),
                    None => None,
                };
                let status = worker.get_last_known_status().await;
                let observation = OwnerAuthoritySnapshot {
                    wallet: owner.snapshot(),
                    generation: worker
                        .published_authority_generation
                        .load(Ordering::Acquire),
                    scope: inputs.scope.clone(),
                    revoked: crate::durable_host::owner_revoked_card_ids(&status),
                    root_states,
                };
                drop(boundary);
                return Ok(Some(observation));
            }
        }
    }

    async fn prepare_source_transfers<Ctx: WorkerCtx>(
        self: &Arc<Self>,
        worker: &Arc<Worker<Ctx>>,
        boundary: Arc<CardEventBoundary>,
    ) -> Result<
        Vec<crate::durable_host::permissions::PendingSourceCardTransferRetry>,
        WorkerExecutorError,
    > {
        let pending = worker
            .get_last_known_status()
            .await
            .pending_card_events
            .clone();
        let mut retries = Vec::new();
        for pending in pending {
            let QueuedCardEvent::TransferStarted(transfer) = &pending.event else {
                continue;
            };
            let Some(mut retry) =
                crate::durable_host::permissions::prepare_pending_source_card_transfer_retry(
                    worker.oplog().as_ref(),
                    &worker.owned_agent_id.agent_id,
                    &pending,
                    transfer,
                )
                .await?
            else {
                continue;
            };
            if !retry.started {
                self.commit(
                    worker,
                    boundary.clone(),
                    OplogEntry::card_transfer_started(
                        retry.entity_parent_start_index(),
                        retry.transfer_id,
                        retry.source_card_id,
                        CardHolder::Agent(AgentCardHolder {
                            agent_id: worker.owned_agent_id.agent_id.clone(),
                        }),
                        CardHolder::Agent(AgentCardHolder {
                            agent_id: retry.target_agent_id.clone(),
                        }),
                        0,
                    ),
                )
                .await?;
                retry.started = true;
            }
            retries.push(retry);
        }
        Ok(retries)
    }

    pub(crate) async fn synchronize_locked<Ctx: WorkerCtx>(
        self: &Arc<Self>,
        worker: &Arc<Worker<Ctx>>,
        boundary: Arc<CardEventBoundary>,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Result<Arc<WalletSnapshot>, WorkerExecutorError> {
        self.validate_boundary(worker, boundary.as_ref()).await?;
        loop {
            let pending = crate::durable_host::next_drainable_card_events(
                worker
                    .get_last_known_status()
                    .await
                    .pending_card_events
                    .clone(),
            );
            let Some(first) = pending.first() else {
                break;
            };
            let mut candidate = None;
            let entry = match &first.event {
                QueuedCardEvent::Revoke(_) => {
                    let parent = first.entity_parent_start_index;
                    let mut revoked_card_ids = pending
                        .iter()
                        .filter(|pending| pending.entity_parent_start_index == parent)
                        .filter_map(|pending| match &pending.event {
                            QueuedCardEvent::Revoke(event) => Some(event.card_id),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    revoked_card_ids.sort_unstable();
                    revoked_card_ids.dedup();
                    OplogEntry::CardRevokedCascade {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: parent,
                        revoked_card_ids,
                        affected_wallets: Vec::new(),
                        local_wallet_generation: 0,
                    }
                }
                QueuedCardEvent::Install(_) | QueuedCardEvent::TransferReceived(_) => {
                    let card = match &first.event {
                        QueuedCardEvent::Install(event) => event.card.clone(),
                        QueuedCardEvent::TransferReceived(event) => event.card.clone(),
                        _ => unreachable!(),
                    }
                    .ok_or_else(|| {
                        WorkerExecutorError::runtime("queued card is missing its payload")
                    })?;
                    let card_id = card.card_id();
                    candidate = Some(self.interest.contribute(None, &[card_id]));
                    let state = worker
                        .card_service()
                        .check_cards(vec![card_id])
                        .await?
                        .remove(&card_id);
                    let failure = match state {
                        Some(CardState::Live(registered)) if registered.as_ref() == &card => None,
                        Some(CardState::Live(_)) => Some(CardInstallFailure::NotPermitted),
                        Some(CardState::Revoked) => Some(CardInstallFailure::CardRevoked),
                        Some(CardState::Unknown) | None => Some(CardInstallFailure::NotFound),
                    };
                    if let Some(failure) = failure {
                        OplogEntry::card_install_failed(
                            first.entity_parent_start_index,
                            first.oplog_index,
                            card_id,
                            failure,
                        )
                    } else {
                        match &first.event {
                            QueuedCardEvent::Install(_) => OplogEntry::card_installed(
                                first.entity_parent_start_index,
                                Some(first.oplog_index),
                                Box::new(card),
                                0,
                            ),
                            QueuedCardEvent::TransferReceived(event) => {
                                OplogEntry::card_transferred(
                                    first.entity_parent_start_index,
                                    event.transfer_id,
                                    event.source_card_id,
                                    card_id,
                                    CardHolder::Agent(AgentCardHolder {
                                        agent_id: self.baseline.owner_id.agent_id.clone(),
                                    }),
                                    Box::new(card),
                                    0,
                                )
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                QueuedCardEvent::TransferStarted(_) => unreachable!("filtered above"),
            };
            self.validate_boundary(worker, boundary.as_ref()).await?;
            let mut prepared = WalletCommit::prepare(boundary.clone(), entry).await?;
            prepared._candidate = candidate;
            worker
                .state_actor
                .append_and_commit_wallet(worker.clone(), prepared)
                .await?;
        }
        let expired = self
            .snapshot()
            .cards
            .iter()
            .filter_map(|(id, card)| {
                card.expires_at()
                    .filter(|at| *at <= chrono::Utc::now())
                    .map(|_| *id)
            })
            .collect::<Vec<_>>();
        for id in expired {
            self.commit(
                worker,
                boundary.clone(),
                OplogEntry::card_expired(entity_parent_start_index, id, 0),
            )
            .await?;
        }
        Ok(self.snapshot())
    }

    pub(crate) fn record_derivation(
        &self,
        parent: Option<OplogIndex>,
        card: StoredCard,
    ) -> Result<(), WorkerExecutorError> {
        let mut derivations = self.derivations.lock().unwrap();
        if derivations.contains_key(&card.card_id()) {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "one runtime-card audit in the invocation",
                format!("duplicate audit for {}", card.card_id()),
            ));
        }
        derivations.insert(
            card.card_id(),
            ReplayDerivationEntry {
                parent,
                evidence: Some(ReplayDerivation {
                    card,
                    receipt: self.latest_replay_receipt(),
                }),
            },
        );
        Ok(())
    }

    pub(crate) fn finish_replayed_invocation(&self) {
        self.derivations
            .lock()
            .unwrap()
            .retain(|_, entry| entry.parent.is_some());
    }

    pub(crate) fn finish_replayed_entity(&self, start_index: OplogIndex) {
        self.entity_prefixes.lock().unwrap().remove(&start_index);
        self.derivations
            .lock()
            .unwrap()
            .retain(|_, entry| entry.parent != Some(start_index));
    }

    pub(crate) fn has_recorded_derivation(&self, card: CardId) -> bool {
        self.derivations.lock().unwrap().contains_key(&card)
    }

    pub(crate) async fn recorded_derivation(
        &self,
        start: OplogIndex,
        parent: Option<OplogIndex>,
        card: &StoredCard,
    ) -> Result<bool, WorkerExecutorError> {
        let (audit_parent, audit) = {
            let mut derivations = self.derivations.lock().unwrap();
            let Some(audit) = derivations.get_mut(&card.card_id()) else {
                return Ok(false);
            };
            let evidence = audit.evidence.take().ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    "unconsumed runtime-card audit",
                    format!("audit for {} was already consumed", card.card_id()),
                )
            })?;
            (audit.parent, evidence)
        };
        if audit.receipt.index <= start || audit_parent != parent || &audit.card != card {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "matching runtime-card audit after the call Start",
                format!("conflicting CardDerived at {}", audit.receipt.index),
            ));
        }
        audit.receipt.result.await?;
        Ok(true)
    }
}
