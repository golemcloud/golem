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

use super::InvocationStrictness;
use super::attribution::InvocationKey;
use super::durability::RetryPolicyScope;
use super::tail_work::TailWorkTracker;
use crate::model::InvocationContext;
use crate::services::card_interest::CardInterestContribution;
use crate::worker::owner_lane::OwnerInvocationId;
use chrono::{DateTime, Utc};
use golem_common::model::OwnedInvocationId;
use golem_common::model::agent::Principal;
use golem_common::model::card::{CardId, EffectiveSurface, ScopeCard, StoredCard};
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::OplogIndex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The authority of one invocation: its scope card and the effective surface derived from
/// the agent's wallet and that card, with the cache state that says whether the surface is
/// current.
#[derive(Clone, Default)]
pub(crate) struct InvocationAuthority {
    /// The scope card the invocation was admitted with.
    pub(crate) scope_card: Option<ScopeCard>,
    /// The validated root cards of the scope card.
    pub(crate) scope_root_cards: BTreeMap<CardId, StoredCard>,
    /// The wallet handles the guest holds for cards of the scope.
    pub(crate) scope_handles: HashMap<u32, Vec<CardId>>,
    /// The Store's interest in card events for the scope's roots.
    pub(crate) scope_interest: Option<Arc<CardInterestContribution>>,
    /// The surface that authorizes the invocation's host calls.
    pub(crate) effective_surface: EffectiveSurface,
    /// Whether `effective_surface` was synchronized with the owner's authority.
    pub(crate) initialized: bool,
    /// The published authority generation `effective_surface` was synchronized at.
    pub(crate) processed_generation: u64,
    /// The earliest expiry among the cards `effective_surface` was derived from.
    pub(crate) next_expiration: Option<DateTime<Utc>>,
    /// Set when the invocation's guest call ends, or when its failure gives up. From then on the
    /// scope card no longer applies, and the invocation's calls have the agent's authority only.
    pub(crate) final_outcome_reached: bool,
}

impl InvocationAuthority {
    /// The scope card that bounds the invocation's calls: none once its final outcome is reached.
    pub(crate) fn bound_scope_card(&self) -> Option<&ScopeCard> {
        self.scope_card
            .as_ref()
            .filter(|_| !self.final_outcome_reached)
    }
}

#[derive(Clone)]
pub(crate) struct InvocationExecutionState {
    /// The `AgentInvocationStarted` index of a primary-Store invocation, once it is recorded.
    pub(crate) start_index: Option<OplogIndex>,
    /// The principal the running export call acts for.
    pub(crate) principal: Option<Principal>,
    /// The ID of the running export call, when it is an invocation with an invocation ID.
    pub(crate) invocation_id: Option<OwnedInvocationId>,
    /// The spans of the invocation's invocation context.
    pub(crate) invocation_context: InvocationContext,
    /// The span that host calls of the invocation open their spans under.
    pub(crate) current_span_id: SpanId,
    /// Whether the invocation is restricted to read-only side effects. When `ReadOnly`,
    /// outgoing HTTP and RPC host calls are trapped before any oplog entry is written.
    pub(crate) strictness: InvocationStrictness,
    /// The agent method invoked under read-only strictness, reported in
    /// `AgentError::ReadOnlyViolation`.
    pub(crate) read_only_method_name: Option<String>,
    /// The fallback retry point: the index attached to an `Error` entry for a trap that happens
    /// outside any in-flight durable call. `begin_function`, transaction begin and the explicit
    /// HTTP and RPC retry-point writes maintain it, so it normally tracks the last persisted side
    /// effect or open scope `Start`. An in-flight durable call carries its own retry grouping in
    /// its call-owned `execution_scope` and in the semantic-trap error marker instead.
    pub(crate) retry_point: OplogIndex,
    /// The outgoing HTTP calls the invocation made live, checked against the plan's
    /// per-invocation limit.
    pub(crate) http_call_count: u64,
    /// The RPC calls the invocation made live, checked against the plan's per-invocation limit.
    pub(crate) rpc_call_count: u64,
    /// Zero-based ordinal of the next invocation-local scope card the invocation derives.
    pub(crate) scope_card_mint_ordinal: u64,
    /// Next tool invocation attempt ordinal in each durable parent namespace.
    pub(crate) tool_invocation_attempt_ordinals: HashMap<OwnerInvocationId, u64>,
    /// The tail work counted on the invocation: the activities of trackers obtained while the
    /// invocation was attributed. It is not carried to the next invocation.
    pub(crate) tail_work: TailWorkTracker,
    /// The retry policy scopes that a task of the invocation had open after its latest scope
    /// change. The retry decision for a guest trap, which names no task, uses them. They are not
    /// carried to the next invocation.
    pub(crate) trap_retry_scopes: Vec<RetryPolicyScope>,
    /// The authority of the invocation.
    pub(crate) authority: InvocationAuthority,
}

impl Default for InvocationExecutionState {
    fn default() -> Self {
        let invocation_context = InvocationContext::new(None);
        let current_span_id = invocation_context.root.span_id().clone();
        Self {
            start_index: None,
            principal: None,
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

pub(crate) struct Invocations {
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
    pub(crate) fn loop_key(&self) -> InvocationKey {
        self.loop_key
    }

    /// Begins the entry of the invocation the loop runs next, replacing the previous
    /// invocation's entry. The new entry starts from the state of the invocation that ran
    /// before, until the loop sets the new invocation's own start index, context and principal.
    pub(crate) fn begin(&mut self, key: InvocationKey) {
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
        if carried.authority.final_outcome_reached {
            // The surface was derived without the scope card; the next authorization derives it
            // again with the card.
            carried.authority.final_outcome_reached = false;
            carried.authority.initialized = false;
        }
        self.entries.insert(key, carried);
        self.entries
            .retain(|existing, _| *existing == InvocationKey::AgentLevel || *existing == key);
        self.loop_key = key;
    }

    /// The invocation whose entry `attributed` reads: `attributed` while its entry lives,
    /// otherwise the loop's invocation. Agent-level work reads the entry of the invocation the
    /// loop runs, as unattributed work does, so the agent-level entry is read only while the loop
    /// runs no invocation.
    pub(crate) fn key(&self, attributed: Option<InvocationKey>) -> InvocationKey {
        attributed
            .filter(|key| *key != InvocationKey::AgentLevel && self.entries.contains_key(key))
            .unwrap_or(self.loop_key)
    }

    pub(crate) fn get(&self, attributed: Option<InvocationKey>) -> &InvocationExecutionState {
        self.entries
            .get(&self.key(attributed))
            .expect("the loop's invocation always has an entry")
    }

    pub(crate) fn get_mut(
        &mut self,
        attributed: Option<InvocationKey>,
    ) -> &mut InvocationExecutionState {
        let key = self.key(attributed);
        self.entries
            .get_mut(&key)
            .expect("the loop's invocation always has an entry")
    }

    /// The entry of exactly the `attributed` invocation while it lives, for state that only the
    /// invocation's own work may write. Work of a replaced invocation gets none, so it never
    /// writes the entry of the invocation the loop runs.
    pub(crate) fn own_mut(
        &mut self,
        attributed: Option<InvocationKey>,
    ) -> Option<&mut InvocationExecutionState> {
        self.entries.get_mut(&attributed?)
    }

    /// Every entry, for changes to state that every invocation derives from the agent's wallet.
    pub(crate) fn entries_mut(&mut self) -> impl Iterator<Item = &mut InvocationExecutionState> {
        self.entries.values_mut()
    }

    pub(crate) fn entries(
        &self,
    ) -> impl Iterator<Item = (&InvocationKey, &InvocationExecutionState)> {
        self.entries.iter()
    }

    /// The entry of exactly `key`, which must live.
    pub(crate) fn entry_mut(&mut self, key: InvocationKey) -> &mut InvocationExecutionState {
        self.entries
            .get_mut(&key)
            .expect("the entry was read from the map")
    }

    /// The entry of the loop's invocation, for the invocation loop's own code.
    pub(crate) fn loop_entry_mut(&mut self) -> &mut InvocationExecutionState {
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

    #[test]
    fn beginning_an_invocation_moves_the_agent_level_authority_into_it() {
        let mut invocations = Invocations::default();
        let first = invocation();
        let second = invocation();

        invocations
            .loop_entry_mut()
            .authority
            .scope_handles
            .insert(7, Vec::new());
        invocations.begin(first);
        assert!(
            invocations.entries[&InvocationKey::AgentLevel]
                .authority
                .scope_handles
                .is_empty()
        );
        assert!(
            invocations.entries[&first]
                .authority
                .scope_handles
                .contains_key(&7)
        );

        // The entity cycle: back to the agent level, then the next invocation.
        invocations.begin(InvocationKey::AgentLevel);
        assert!(
            invocations.entries[&InvocationKey::AgentLevel]
                .authority
                .scope_handles
                .contains_key(&7)
        );
        invocations.begin(second);
        assert!(
            invocations.entries[&InvocationKey::AgentLevel]
                .authority
                .scope_handles
                .is_empty(),
            "a finished invocation's scope handles stay only in the running invocation's entry"
        );
        assert!(
            invocations.entries[&second]
                .authority
                .scope_handles
                .contains_key(&7)
        );
    }

    #[test]
    fn beginning_after_the_final_outcome_derives_the_surface_again() {
        let mut invocations = Invocations::default();
        let first = invocation();
        invocations.begin(first);
        invocations.loop_entry_mut().authority.initialized = true;
        invocations.loop_entry_mut().authority.final_outcome_reached = true;
        invocations.begin(invocation());
        let authority = &invocations.get(None).authority;
        assert!(!authority.final_outcome_reached);
        assert!(!authority.initialized);
    }
}
