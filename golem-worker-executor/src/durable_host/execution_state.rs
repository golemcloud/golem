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
use crate::model::InvocationContext;
use crate::worker::owner_lane::OwnerInvocationId;
use golem_common::model::OwnedInvocationId;
use golem_common::model::agent::Principal;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::OplogIndex;
use std::collections::HashMap;

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
        let carried = if self.loop_key == InvocationKey::AgentLevel {
            self.loop_entry().clone()
        } else {
            self.entries
                .remove(&self.loop_key)
                .expect("the loop's invocation always has an entry")
        };
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

    /// The entry of the loop's invocation, for the invocation loop's own code.
    pub(crate) fn loop_entry(&self) -> &InvocationExecutionState {
        self.get(None)
    }

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

        // Once the loop runs no invocation, the agent-level entry starts from the last one.
        invocations.begin(InvocationKey::AgentLevel);
        assert_eq!(invocations.key(Some(second)), InvocationKey::AgentLevel);
        assert_eq!(
            invocations.get(None).start_index,
            Some(OplogIndex::from_u64(9))
        );
    }
}
