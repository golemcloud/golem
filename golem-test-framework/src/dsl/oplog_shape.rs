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

use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use std::collections::{BTreeMap, BTreeSet};

/// Checks a completed history. Exceptions name the exact Start indices left incomplete by
/// an injected crash/cancellation, or completed later by a retained call/destructor.
pub fn assert_settled_oplog(
    entries: &[PublicOplogEntryWithIndex],
    incomplete_starts: &[OplogIndex],
    retained_terminals: &[OplogIndex],
) {
    let mut finished = BTreeSet::new();
    let mut current = None;
    let mut boundaries = BTreeMap::new();
    let mut roots = BTreeMap::new();
    let mut starts = BTreeMap::new();
    let mut terminals = BTreeMap::new();
    for entry in entries {
        let index = entry.oplog_index;
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(_) => {
                current = Some(index);
                roots.insert(index, index);
            }
            PublicOplogEntry::AgentInvocationFinished(params) => {
                assert!(
                    finished.insert(params.invocation_id),
                    "duplicate Finished for {}",
                    params.invocation_id
                );
                boundaries.insert(current.expect("Finished has an invocation Start"), index);
            }
            PublicOplogEntry::AgentInvocationFailed(_) => {
                boundaries.insert(current.expect("Failed has an invocation Start"), index);
            }
            PublicOplogEntry::AgentInvocationSettled(_) => {
                boundaries.insert(current.expect("Settled has an invocation Start"), index);
            }
            PublicOplogEntry::Start(params) => {
                let root = params
                    .driver
                    .or_else(|| {
                        params
                            .invocation_id
                            .and_then(|id| roots.get(&id.anchor).copied())
                    })
                    .or_else(|| {
                        params
                            .parent_start_index
                            .and_then(|parent| roots.get(&parent).copied())
                    });
                if let Some(root) = root {
                    roots.insert(index, root);
                }
                starts.insert(index, root);
            }
            PublicOplogEntry::End(params) => {
                assert!(
                    terminals.insert(params.start_index, index).is_none(),
                    "duplicate terminal for Start {}",
                    params.start_index
                );
            }
            PublicOplogEntry::Cancelled(params) => {
                assert!(
                    terminals.insert(params.start_index, index).is_none(),
                    "duplicate terminal for Start {}",
                    params.start_index
                );
            }
            _ => {}
        }
    }
    for (start, root) in starts {
        let terminal = terminals.get(&start);
        assert_eq!(
            terminal.is_none(),
            incomplete_starts.contains(&start),
            "unexpected open or closed Start at {start}"
        );
        if let Some(boundary) = root.and_then(|root| boundaries.get(&root)) {
            assert!(
                start < *boundary,
                "positional Start {start} after invocation terminal {boundary}"
            );
            if let Some(terminal) = terminal {
                assert!(
                    *terminal < *boundary || retained_terminals.contains(&start),
                    "terminal {terminal} for positional Start {start} after invocation terminal {boundary}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::caller::{Caller, ClaimVisibility, PublicCaller};
    use golem_common::model::card::{PublicInvocationWalletPin, WalletVersionToken};
    use golem_common::model::oplog::public_oplog_entry::{
        AgentInvocationFailedParams, AgentInvocationSettledParams, AgentInvocationStartedParams,
        EndParams, SaveSnapshotParameters, StartParams,
    };
    use golem_common::model::oplog::{
        DurableFunctionType, PublicAgentInvocation, PublicOplogEntryAttribution,
    };
    use golem_common::model::{InvocationId, Timestamp};
    use test_r::test;

    #[test]
    #[should_panic(expected = "after invocation terminal")]
    fn unnumbered_scope_cannot_end_after_settlement() {
        let timestamp = Timestamp::now_utc();
        let invocation_id = InvocationId::new();
        let root = OplogIndex::from_u64(1);
        let scope = OplogIndex::from_u64(2);
        let history = vec![
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                timestamp,
                invocation: PublicAgentInvocation::SaveSnapshot(SaveSnapshotParameters {
                    caller: PublicCaller::new(&Caller::anonymous(), ClaimVisibility::Hidden),
                }),
                wallet_pin: PublicInvocationWalletPin {
                    wallet_token: WalletVersionToken {
                        wallet_id_hash: [0; 32],
                        generation: 0,
                    },
                    scope_card_id: None,
                },
            }),
            PublicOplogEntry::Start(StartParams {
                timestamp,
                parent_start_index: Some(root),
                function_name: "scope".into(),
                invocation_id: None,
                observational_owner: None,
                driver: None,
                request: None,
                durable_function_type: DurableFunctionType::WriteLocal.into(),
                span_started: None,
                denial: None,
            }),
            PublicOplogEntry::AgentInvocationFailed(AgentInvocationFailedParams {
                timestamp,
                invocation_id,
                error: "snapshot failed".into(),
                start_index: root,
            }),
            PublicOplogEntry::AgentInvocationSettled(AgentInvocationSettledParams {
                timestamp,
                invocation_id,
                consumed_fuel: 0,
            }),
            PublicOplogEntry::End(EndParams {
                timestamp,
                start_index: scope,
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            }),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, entry)| PublicOplogEntryWithIndex {
            oplog_index: OplogIndex::from_u64(index as u64 + 1),
            attribution: PublicOplogEntryAttribution::agent(),
            entry,
        })
        .collect::<Vec<_>>();
        assert_settled_oplog(&history, &[], &[]);
    }
}
