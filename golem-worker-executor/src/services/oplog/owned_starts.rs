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

use golem_common::model::oplog::{OplogEntry, OplogIndex};

/// Folds the entries an oplog writer appends to check that every `Start` appended while an agent
/// invocation is open records an owner: a driver, an invocation id, a parent or an observational
/// owner. Replay reports such a `Start` without any of them as a divergence.
///
/// An invocation opens at its `AgentInvocationStarted` and closes at its `AgentInvocationSettled`
/// or at a terminal that awaits no settlement. A writer that has appended no
/// `AgentInvocationStarted` yet does not know whether one is open, and checks nothing.
#[derive(Debug, Default)]
pub(super) struct OwnedStartCheck {
    invocation_open: bool,
}

impl OwnedStartCheck {
    /// Checks the entry appended at `index`, then folds it.
    pub(super) fn append(&mut self, index: OplogIndex, entry: &OplogEntry) {
        debug_assert!(
            !(self.invocation_open
                && matches!(
                    entry,
                    OplogEntry::Start {
                        driver: None,
                        invocation_id: None,
                        parent_start_index: None,
                        observational_owner: None,
                        ..
                    }
                )),
            "the Start appended at {index} records no driver, invocation id, parent or \
             observational owner while an invocation is open"
        );
        match entry {
            OplogEntry::AgentInvocationStarted { .. } => self.invocation_open = true,
            OplogEntry::AgentInvocationSettled { .. }
            | OplogEntry::AgentInvocationFinished {
                awaits_settlement: false,
                ..
            }
            | OplogEntry::AgentInvocationFailed {
                entity_parent_start_index: None,
                awaits_settlement: false,
                ..
            } => self.invocation_open = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OwnedStartCheck;
    use golem_common::model::Timestamp;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::{DurableFunctionType, OplogEntry, OplogIndex};
    use test_r::test;

    fn start(observational_owner: Option<u64>) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::Custom("<scope:transaction>".to_string()),
            invocation_id: None,
            observational_owner: observational_owner.map(OplogIndex::from_u64),
            driver: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: None,
        }
    }

    fn open() -> OwnedStartCheck {
        OwnedStartCheck {
            invocation_open: true,
        }
    }

    #[test]
    fn a_start_with_an_owner_appends_while_an_invocation_is_open() {
        open().append(OplogIndex::from_u64(7), &start(Some(3)));
    }

    #[test]
    fn a_start_without_an_owner_appends_outside_an_invocation() {
        OwnedStartCheck::default().append(OplogIndex::from_u64(7), &start(None));
        let mut check = open();
        check.append(
            OplogIndex::from_u64(6),
            &OplogEntry::agent_invocation_settled(uuid::Uuid::from_u128(1).into(), 0),
        );
        check.append(OplogIndex::from_u64(7), &start(None));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "records no driver, invocation id, parent or observational owner")]
    fn a_start_without_an_owner_is_refused_while_an_invocation_is_open() {
        open().append(OplogIndex::from_u64(7), &start(None));
    }
}
