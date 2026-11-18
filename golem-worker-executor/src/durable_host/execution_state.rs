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

//! Execution state that belongs to one invocation of a Store, keyed by its [`InvocationKey`].
//!
//! Host code reads the entry of the invocation it is attributed to (see [`super::attribution`]).
//! Code that no invocation is attributed to, which is the invocation loop's own code, agent-level
//! work, and work of an invocation whose entry is gone, such as a task parked past its
//! invocation, read the entry of the invocation the loop runs or last ran.
//!
//! The invocation loop runs one invocation at a time, so at most one invocation entry lives
//! next to the [`InvocationKey::AgentLevel`] entry: beginning an invocation replaces the entry
//! of the previous one.

use super::attribution::InvocationKey;
use super::durability::RetryPolicyScope;
use super::tail_work::TailWorkTracker;
use super::{InvocationStrictness, authority_snapshot_is_current_at};
use crate::model::InvocationContext;
use crate::services::card_interest::CardInterestContribution;
use crate::worker::owner_lane::OwnerInvocationId;
use chrono::{DateTime, Utc};
use golem_common::model::OwnedInvocationId;
use golem_common::model::caller::Caller;
use golem_common::model::card::{CardId, EffectiveSurface, ScopeCard, StoredCard};
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::OplogIndex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// The authority of one invocation: its scope card and the effective surface derived from
/// the agent's wallet and that card, with the cache state that says whether the surface is
/// current.
#[derive(Clone, Default)]
pub struct InvocationAuthority {
    scope: ScopeBinding,
    /// The validated root cards of the scope card.
    root_cards: BTreeMap<CardId, StoredCard>,
    /// The wallet handles the guest holds for cards of the scope, with the root cards each
    /// handle's card depends on. A handle whose roots stay live outlives a removed scope card.
    handles: HashMap<u32, Vec<CardId>>,
    /// The Store's interest in card events for the scope's roots.
    interest: Option<Arc<CardInterestContribution>>,
    /// The surface that authorizes the invocation's host calls.
    surface: EffectiveSurface,
    synchronization: Synchronization,
    /// The earliest expiry among the cards `surface` was derived from.
    next_expiration: Option<DateTime<Utc>>,
}

/// Whether the scope card the invocation was admitted with bounds its calls.
#[derive(Clone, Default)]
enum ScopeBinding {
    /// The invocation has no scope card.
    #[default]
    Unscoped,
    /// The scope card bounds the invocation's calls.
    Bound(ScopeCard),
    /// The invocation's guest call ended, or its failure gave up: its calls have the agent's
    /// authority only. The card it was admitted with stays until the scope is replaced or
    /// cleared, or a root of the card is revoked. When the loop begins the next invocation from
    /// this entry, the card bounds the calls again and the surface is stale.
    FinalOutcome { admitted: Option<ScopeCard> },
}

impl ScopeBinding {
    fn card(&self) -> Option<&ScopeCard> {
        match self {
            Self::Unscoped => None,
            Self::Bound(card) => Some(card),
            Self::FinalOutcome { admitted } => admitted.as_ref(),
        }
    }

    /// The binding once its card is removed: a reached final outcome stays reached.
    fn without_card(&self) -> Self {
        match self {
            Self::Unscoped | Self::Bound(_) => Self::Unscoped,
            Self::FinalOutcome { .. } => Self::FinalOutcome { admitted: None },
        }
    }
}

/// Whether the surface is synchronized with the owner's authority.
#[derive(Clone, Copy)]
enum Synchronization {
    /// The surface must be synchronized again before it authorizes from the cache: what it is
    /// derived from changed, or card authority was closed when it was synchronized at
    /// `last_generation`.
    Stale { last_generation: u64 },
    /// The surface was synchronized at this published authority generation.
    Synchronized { generation: u64 },
}

impl Default for Synchronization {
    fn default() -> Self {
        Self::Stale { last_generation: 0 }
    }
}

/// What removing an invocation's scope card and handles removed.
pub struct ScopeRemoval {
    /// Whether the scope card was removed.
    pub card_removed: bool,
    /// The removed wallet handles, which the caller retires in the resource table.
    pub handles: HashSet<u32>,
}

impl InvocationAuthority {
    /// The scope card that bounds the invocation's calls: none once its final outcome is reached.
    pub fn bound_scope_card(&self) -> Option<&ScopeCard> {
        match &self.scope {
            ScopeBinding::Bound(card) => Some(card),
            ScopeBinding::Unscoped | ScopeBinding::FinalOutcome { .. } => None,
        }
    }

    /// The scope card the invocation was admitted with, also once its final outcome is reached.
    pub fn admitted_scope_card(&self) -> Option<&ScopeCard> {
        self.scope.card()
    }

    pub fn scope_root_cards(&self) -> &BTreeMap<CardId, StoredCard> {
        &self.root_cards
    }

    pub fn scope_interest(&self) -> Option<&Arc<CardInterestContribution>> {
        self.interest.as_ref()
    }

    pub fn effective_surface(&self) -> &EffectiveSurface {
        &self.surface
    }

    pub fn next_expiration(&self) -> Option<DateTime<Utc>> {
        self.next_expiration
    }

    /// The published authority generation the surface was last synchronized at, also once it is
    /// stale.
    pub fn synchronized_generation(&self) -> u64 {
        match self.synchronization {
            Synchronization::Stale { last_generation } => last_generation,
            Synchronization::Synchronized { generation } => generation,
        }
    }

    /// Whether the cached surface authorizes at `now` without synchronizing again: it is
    /// synchronized at `published_generation`, card authority is open, and none of its cards
    /// expired.
    pub fn is_current_at(
        &self,
        authority_open: bool,
        published_generation: u64,
        now: DateTime<Utc>,
    ) -> bool {
        authority_snapshot_is_current_at(
            matches!(self.synchronization, Synchronization::Synchronized { .. }),
            authority_open,
            self.synchronized_generation(),
            published_generation,
            self.next_expiration,
            now,
        )
    }

    /// Replaces the surface with one the caller derived for the invocation.
    pub fn replace_surface(&mut self, surface: EffectiveSurface) {
        self.surface = surface;
    }

    pub fn set_next_expiration(&mut self, next_expiration: Option<DateTime<Utc>>) {
        self.next_expiration = next_expiration;
    }

    /// Marks the surface as synchronized with the owner's authority at `generation`.
    pub fn synchronize_at(&mut self, generation: u64) {
        self.synchronization = Synchronization::Synchronized { generation };
    }

    /// Marks the surface as stale, so the next authorization synchronizes it again.
    pub fn invalidate(&mut self) {
        self.synchronization = Synchronization::Stale {
            last_generation: self.synchronized_generation(),
        };
    }

    /// Installs the scope the invocation is admitted with, replacing the previous scope. Returns
    /// the previous scope's wallet handles, which the caller retires in the resource table.
    pub fn install_scope(
        &mut self,
        card: Option<ScopeCard>,
        root_cards: Vec<StoredCard>,
        interest: Option<Arc<CardInterestContribution>>,
    ) -> HashSet<u32> {
        let previous = self.clear_scope();
        self.scope = card.map_or(ScopeBinding::Unscoped, ScopeBinding::Bound);
        self.root_cards = root_cards
            .into_iter()
            .map(|card| (card.card_id(), card))
            .collect();
        self.interest = interest;
        previous.handles
    }

    /// Removes the scope card with its root cards, the wallet handles and the card interest, and
    /// marks the surface as stale. A reached final outcome stays reached.
    pub fn clear_scope(&mut self) -> ScopeRemoval {
        self.invalidate();
        let card_removed = self.scope.card().is_some();
        self.scope = self.scope.without_card();
        if card_removed {
            self.root_cards.clear();
        }
        self.interest = None;
        ScopeRemoval {
            card_removed,
            handles: std::mem::take(&mut self.handles).into_keys().collect(),
        }
    }

    /// Removes the scope card if `revoked_card_ids` include one of its roots, and the wallet
    /// handles whose cards depend on one of them.
    pub fn remove_scope_for_revoked_roots(&mut self, revoked_card_ids: &[CardId]) -> ScopeRemoval {
        let card_removed = self.scope.card().is_some_and(|card| {
            card.root_card_ids
                .iter()
                .any(|root_id| revoked_card_ids.contains(root_id))
        });
        if card_removed {
            self.scope = self.scope.without_card();
            self.root_cards.clear();
        }
        let handles = self
            .handles
            .iter()
            .filter(|(_, root_card_ids)| {
                root_card_ids
                    .iter()
                    .any(|root_id| revoked_card_ids.contains(root_id))
            })
            .map(|(rep, _)| *rep)
            .collect::<HashSet<_>>();
        self.handles.retain(|rep, _| !handles.contains(rep));
        ScopeRemoval {
            card_removed,
            handles,
        }
    }

    /// Replaces the scope's root cards with the live ones that replay or a liveness check found.
    pub fn replace_scope_root_cards(&mut self, root_cards: BTreeMap<CardId, StoredCard>) {
        self.root_cards = root_cards;
    }

    /// Replaces the card interest, for the scope an entity Store inherits.
    pub fn replace_scope_interest(&mut self, interest: Arc<CardInterestContribution>) {
        self.interest = Some(interest);
    }

    /// Records a wallet handle the guest holds for a card that depends on `root_card_ids`.
    pub fn hold_scope_handle(&mut self, rep: u32, root_card_ids: Vec<CardId>) {
        self.handles.insert(rep, root_card_ids);
    }

    /// Forgets a wallet handle the guest dropped or the host took back.
    pub fn forget_scope_handle(&mut self, rep: u32) {
        self.handles.remove(&rep);
    }

    /// Releases the scope card once the invocation's guest call ends or its failure gives up.
    pub fn reach_final_outcome(&mut self) {
        self.scope = match std::mem::take(&mut self.scope) {
            ScopeBinding::Unscoped => ScopeBinding::FinalOutcome { admitted: None },
            ScopeBinding::Bound(card) => ScopeBinding::FinalOutcome {
                admitted: Some(card),
            },
            reached @ ScopeBinding::FinalOutcome { .. } => reached,
        };
    }

    /// Binds the admitted scope card again for the next invocation, after a reached final
    /// outcome. The surface was derived without the card, so it is stale.
    fn rebind_scope(&mut self) {
        self.scope = match std::mem::take(&mut self.scope) {
            ScopeBinding::FinalOutcome { admitted } => {
                self.invalidate();
                admitted.map_or(ScopeBinding::Unscoped, ScopeBinding::Bound)
            }
            unchanged => unchanged,
        };
    }
}

#[derive(Clone)]
pub struct InvocationExecutionState {
    /// The `AgentInvocationStarted` index of a primary-Store invocation, once it is recorded.
    pub start_index: Option<OplogIndex>,
    /// The caller of the running export call. The executor reads it through
    /// `invocation_caller`, which falls back to the agent itself in both roles.
    pub caller: Option<Caller>,
    /// The ID of the running export call, when it is an invocation with an invocation ID.
    pub invocation_id: Option<OwnedInvocationId>,
    /// The spans of the invocation's invocation context.
    pub invocation_context: InvocationContext,
    /// The span that host calls of the invocation open their spans under.
    pub current_span_id: SpanId,
    /// Whether the invocation is restricted to read-only side effects. When `ReadOnly`,
    /// outgoing HTTP and RPC host calls are trapped before any oplog entry is written.
    pub strictness: InvocationStrictness,
    /// The agent method invoked under read-only strictness, reported in
    /// `AgentError::ReadOnlyViolation`.
    pub read_only_method_name: Option<String>,
    /// The fallback retry point: the index attached to an `Error` entry for a trap that happens
    /// outside any in-flight durable call. `begin_function`, transaction begin and the explicit
    /// HTTP and RPC retry-point writes maintain it, so it normally tracks the last persisted side
    /// effect or open scope `Start`. An in-flight durable call carries its own retry grouping in
    /// its call-owned `execution_scope` and in the semantic-trap error marker instead.
    pub retry_point: OplogIndex,
    /// The outgoing HTTP calls the invocation made live, checked against the plan's
    /// per-invocation limit.
    pub http_call_count: u64,
    /// The RPC calls the invocation made live, checked against the plan's per-invocation limit.
    pub rpc_call_count: u64,
    /// Zero-based ordinal of the next invocation-local scope card the invocation derives.
    pub scope_card_mint_ordinal: u64,
    /// Next tool invocation attempt ordinal in each durable parent namespace.
    pub tool_invocation_attempt_ordinals: HashMap<OwnerInvocationId, u64>,
    /// The tail work counted on the invocation: the activities of trackers obtained while the
    /// invocation was attributed. It is not carried to the next invocation.
    pub tail_work: TailWorkTracker,
    /// The retry policy scopes that a task of the invocation had open after its latest scope
    /// change. The retry decision for a guest trap, which names no task, uses them. They are not
    /// carried to the next invocation.
    pub trap_retry_scopes: Vec<RetryPolicyScope>,
    /// The authority of the invocation.
    pub authority: InvocationAuthority,
}

impl Default for InvocationExecutionState {
    fn default() -> Self {
        let invocation_context = InvocationContext::new(None);
        let current_span_id = invocation_context.root.span_id().clone();
        Self {
            start_index: None,
            caller: None,
            invocation_id: None,
            invocation_context,
            current_span_id,
            strictness: InvocationStrictness::Normal,
            read_only_method_name: None,
            retry_point: OplogIndex::INITIAL,
            http_call_count: 0,
            rpc_call_count: 0,
            scope_card_mint_ordinal: 0,
            tool_invocation_attempt_ordinals: HashMap::new(),
            tail_work: TailWorkTracker::default(),
            trap_retry_scopes: Vec::new(),
            authority: InvocationAuthority::default(),
        }
    }
}

pub struct Invocations {
    loop_key: InvocationKey,
    entries: HashMap<InvocationKey, InvocationExecutionState>,
}

impl Default for Invocations {
    fn default() -> Self {
        Self {
            loop_key: InvocationKey::AgentLevel,
            entries: HashMap::from([(InvocationKey::AgentLevel, Default::default())]),
        }
    }
}

impl Invocations {
    /// The invocation the loop runs, or last ran.
    pub fn loop_key(&self) -> InvocationKey {
        self.loop_key
    }

    /// Begins the entry of the invocation the loop runs next, replacing the previous
    /// invocation's entry. The new entry starts from the state of the invocation that ran
    /// before, until the loop sets the new invocation's own start index, context and caller.
    pub fn begin(&mut self, key: InvocationKey) {
        if key == self.loop_key {
            return;
        }
        let mut carried = if self.loop_key == InvocationKey::AgentLevel {
            // The authority moves to the new entry: a scope card, its handles and its card
            // interest belong to one invocation, and nothing reads the agent-level authority
            // while the loop runs an invocation.
            let agent_level = self
                .entries
                .get_mut(&InvocationKey::AgentLevel)
                .expect("the agent-level entry always exists");
            let authority = std::mem::take(&mut agent_level.authority);
            InvocationExecutionState {
                authority,
                ..agent_level.clone()
            }
        } else {
            self.entries
                .remove(&self.loop_key)
                .expect("the loop's invocation always has an entry")
        };
        carried.tail_work = TailWorkTracker::default();
        carried.trap_retry_scopes.clear();
        carried.authority.rebind_scope();
        self.entries.insert(key, carried);
        self.entries
            .retain(|existing, _| *existing == InvocationKey::AgentLevel || *existing == key);
        self.loop_key = key;
    }

    /// The invocation whose entry `attributed` reads: `attributed` while its entry lives,
    /// otherwise the loop's invocation. Agent-level work reads the entry of the invocation the
    /// loop runs, as unattributed work does, so the agent-level entry is read only while the loop
    /// runs no invocation.
    pub fn key(&self, attributed: Option<InvocationKey>) -> InvocationKey {
        attributed
            .filter(|key| *key != InvocationKey::AgentLevel && self.entries.contains_key(key))
            .unwrap_or(self.loop_key)
    }

    pub fn get(&self, attributed: Option<InvocationKey>) -> &InvocationExecutionState {
        self.entries
            .get(&self.key(attributed))
            .expect("the loop's invocation always has an entry")
    }

    pub fn get_mut(&mut self, attributed: Option<InvocationKey>) -> &mut InvocationExecutionState {
        let key = self.key(attributed);
        self.entries
            .get_mut(&key)
            .expect("the loop's invocation always has an entry")
    }

    /// The entry of exactly the `attributed` invocation while it lives, for state that only the
    /// invocation's own work may write. Work of a replaced invocation gets none, so it never
    /// writes the entry of the invocation the loop runs.
    pub fn own_mut(
        &mut self,
        attributed: Option<InvocationKey>,
    ) -> Option<&mut InvocationExecutionState> {
        self.entries.get_mut(&attributed?)
    }

    /// Every entry, for changes to state that every invocation derives from the agent's wallet.
    pub fn entries_mut(&mut self) -> impl Iterator<Item = &mut InvocationExecutionState> {
        self.entries.values_mut()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&InvocationKey, &InvocationExecutionState)> {
        self.entries.iter()
    }

    /// The entry of exactly `key`, which must live.
    pub fn entry_mut(&mut self, key: InvocationKey) -> &mut InvocationExecutionState {
        self.entries
            .get_mut(&key)
            .expect("the entry was read from the map")
    }

    /// The entry of the loop's invocation, for the invocation loop's own code.
    pub fn loop_entry_mut(&mut self) -> &mut InvocationExecutionState {
        self.get_mut(None)
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use golem_common::model::InvocationId;

    fn invocation() -> InvocationKey {
        InvocationKey::Invocation(InvocationId(uuid::Uuid::new_v4()))
    }

    #[test]
    fn work_of_a_replaced_invocation_reads_the_entry_of_the_running_one() {
        let mut invocations = Invocations::default();
        let first = invocation();
        let second = invocation();

        invocations.begin(first);
        invocations.loop_entry_mut().start_index = Some(OplogIndex::from_u64(5));
        invocations.begin(second);

        // The second invocation inherits the anchor until its own start is recorded.
        assert_eq!(invocations.key(Some(first)), second);
        assert_eq!(
            invocations.get(Some(second)).start_index,
            Some(OplogIndex::from_u64(5))
        );

        invocations.get_mut(Some(second)).start_index = Some(OplogIndex::from_u64(9));
        assert_eq!(
            invocations.get(Some(first)).start_index,
            Some(OplogIndex::from_u64(9))
        );
        assert_eq!(invocations.key(None), second);

        // Agent-level work reads the running invocation's entry.
        assert_eq!(invocations.key(Some(InvocationKey::AgentLevel)), second);

        // Work of the replaced invocation writes no invocation-owned state of the running one.
        assert!(invocations.own_mut(Some(first)).is_none());
        assert!(invocations.own_mut(None).is_none());
        assert!(invocations.own_mut(Some(second)).is_some());

        // Once the loop runs no invocation, the agent-level entry starts from the last one.
        invocations.begin(InvocationKey::AgentLevel);
        assert_eq!(invocations.key(Some(second)), InvocationKey::AgentLevel);
        assert_eq!(
            invocations.get(None).start_index,
            Some(OplogIndex::from_u64(9))
        );
    }

    fn scope_card(root_card_ids: Vec<CardId>) -> ScopeCard {
        ScopeCard {
            scope_card_id: CardId(uuid::Uuid::from_u128(100)),
            root_card_ids,
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            expires_at: None,
        }
    }

    #[test]
    fn beginning_an_invocation_moves_the_agent_level_authority_into_it() {
        let mut invocations = Invocations::default();
        let first = invocation();
        let second = invocation();

        invocations
            .loop_entry_mut()
            .authority
            .hold_scope_handle(7, Vec::new());
        invocations.begin(first);
        assert!(
            invocations.entries[&InvocationKey::AgentLevel]
                .authority
                .handles
                .is_empty()
        );
        assert!(
            invocations.entries[&first]
                .authority
                .handles
                .contains_key(&7)
        );

        // The entity cycle: back to the agent level, then the next invocation.
        invocations.begin(InvocationKey::AgentLevel);
        assert!(
            invocations.entries[&InvocationKey::AgentLevel]
                .authority
                .handles
                .contains_key(&7)
        );
        invocations.begin(second);
        assert!(
            invocations.entries[&InvocationKey::AgentLevel]
                .authority
                .handles
                .is_empty(),
            "a finished invocation's scope handles stay only in the running invocation's entry"
        );
        assert!(
            invocations.entries[&second]
                .authority
                .handles
                .contains_key(&7)
        );
    }

    #[test]
    fn beginning_after_an_unscoped_final_outcome_makes_the_surface_stale() {
        let mut invocations = Invocations::default();
        invocations.begin(invocation());
        let authority = &mut invocations.loop_entry_mut().authority;
        authority.synchronize_at(4);
        authority.reach_final_outcome();
        let _ = authority.clear_scope();
        authority.synchronize_at(4);

        invocations.begin(invocation());
        let authority = &invocations.get(None).authority;
        assert!(authority.bound_scope_card().is_none());
        assert!(!authority.is_current_at(true, 4, Utc::now()));
    }

    #[test]
    fn beginning_after_the_final_outcome_binds_the_scope_card_again() {
        let mut invocations = Invocations::default();
        invocations.begin(invocation());
        let card = scope_card(vec![CardId(uuid::Uuid::from_u128(1))]);
        let authority = &mut invocations.loop_entry_mut().authority;
        let _ = authority.install_scope(Some(card.clone()), Vec::new(), None);
        authority.synchronize_at(4);
        authority.reach_final_outcome();
        assert!(authority.bound_scope_card().is_none());
        assert!(authority.admitted_scope_card().is_some());

        invocations.begin(invocation());
        let authority = &invocations.get(None).authority;
        assert_eq!(authority.bound_scope_card(), Some(&card));
        assert!(
            !authority.is_current_at(true, 4, Utc::now()),
            "the surface was derived without the card"
        );
        assert_eq!(authority.synchronized_generation(), 4);
    }

    #[test]
    fn an_unrelated_revocation_keeps_the_scope_and_its_handles() {
        let scope_root = CardId(uuid::Uuid::from_u128(1));
        let unrelated = CardId(uuid::Uuid::from_u128(2));
        let mut authority = InvocationAuthority::default();
        let _ = authority.install_scope(Some(scope_card(vec![scope_root])), Vec::new(), None);
        authority.hold_scope_handle(7, vec![scope_root]);

        let removal = authority.remove_scope_for_revoked_roots(&[unrelated]);
        assert!(!removal.card_removed);
        assert!(removal.handles.is_empty());
        assert!(authority.bound_scope_card().is_some());
        assert_eq!(authority.handles, HashMap::from([(7, vec![scope_root])]));
    }

    #[test]
    fn a_revoked_root_removes_the_scope_and_only_its_dependent_handles() {
        let revoked_root = CardId(uuid::Uuid::from_u128(1));
        let unrelated_root = CardId(uuid::Uuid::from_u128(2));
        let mut authority = InvocationAuthority::default();
        let _ = authority.install_scope(Some(scope_card(vec![revoked_root])), Vec::new(), None);
        authority.hold_scope_handle(7, vec![revoked_root]);
        authority.hold_scope_handle(11, vec![unrelated_root]);
        authority.hold_scope_handle(13, vec![revoked_root, unrelated_root]);

        let removal = authority.remove_scope_for_revoked_roots(&[revoked_root]);
        assert!(removal.card_removed);
        assert_eq!(removal.handles, HashSet::from([7, 13]));
        assert!(authority.admitted_scope_card().is_none());
        assert_eq!(
            authority.handles,
            HashMap::from([(11, vec![unrelated_root])])
        );
    }

    #[test]
    fn clearing_the_scope_removes_the_card_and_every_handle() {
        let mut authority = InvocationAuthority::default();
        let _ = authority.install_scope(
            Some(scope_card(vec![CardId(uuid::Uuid::from_u128(1))])),
            Vec::new(),
            None,
        );
        authority.hold_scope_handle(7, vec![CardId(uuid::Uuid::from_u128(1))]);
        authority.hold_scope_handle(11, vec![CardId(uuid::Uuid::from_u128(2))]);
        authority.reach_final_outcome();

        let removal = authority.clear_scope();
        assert!(removal.card_removed, "a released card is removed too");
        assert_eq!(removal.handles, HashSet::from([7, 11]));
        assert!(authority.admitted_scope_card().is_none());
        assert!(authority.handles.is_empty());
    }
}
