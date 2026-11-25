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

use super::super::error::ShardManagerError;
use super::super::model::{ExecutorAddr, ExecutorId, PendingGrant, ShardEpoch, ShardLeaseState};
use super::super::rebalancing::Rebalance;
use golem_common::model::ShardId;
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

pub(super) enum Registration {
    Added,
    Refreshed,
    Replaced {
        previous: ExecutorId,
        push_to: ExecutorId,
    },
}

pub(super) struct RegistrationDecision {
    pub registration: Registration,
    pub number_of_shards: usize,
    pub pending: PendingGrant,
}

pub(super) fn register(
    mut state: ShardLeaseState,
    executor_id: ExecutorId,
    addr: ExecutorAddr,
    pod_name: Option<String>,
    now: Timestamp,
    lease_ttl: Duration,
) -> Result<(ShardLeaseState, RegistrationDecision), ShardManagerError> {
    let already_known = state.has_executor(executor_id);
    let replaced = state.add_executor(executor_id, addr, pod_name, now, lease_ttl);
    let registration = match replaced {
        Some(previous) => Registration::Replaced {
            previous,
            push_to: executor_id,
        },
        None if already_known => Registration::Refreshed,
        None => Registration::Added,
    };
    let pending = state.lease_grant_for(executor_id).ok_or_else(|| {
        ShardManagerError::Internal(format!(
            "executor {executor_id} holds no lease right after being registered"
        ))
    })?;
    let decision = RegistrationDecision {
        registration,
        number_of_shards: state.number_of_shards,
        pending,
    };
    Ok((state, decision))
}

pub(super) fn reap_expired(
    mut state: ShardLeaseState,
    now: Timestamp,
) -> (ShardLeaseState, Vec<(ExecutorId, BTreeSet<ShardId>)>) {
    let expired = state.housekeep(now);
    (state, expired)
}

pub(super) struct RenewalDecision {
    pub pending: PendingGrant,
    pub mismatched: Vec<ShardId>,
}

pub(super) fn renew(
    mut state: ShardLeaseState,
    executor_id: ExecutorId,
    claimed: &BTreeMap<ShardId, ShardEpoch>,
    now: Timestamp,
    lease_ttl: Duration,
) -> Result<(ShardLeaseState, RenewalDecision), ShardManagerError> {
    if !state.has_executor(executor_id) {
        return Err(ShardManagerError::ShardLeaseNotFound { executor_id });
    }

    let mismatched = claimed
        .iter()
        .filter(|(shard_id, provided)| {
            state
                .shard_assignments
                .get(*shard_id)
                .filter(|entry| entry.executor_id == executor_id)
                .map(|entry| entry.epoch)
                != Some(**provided)
        })
        .map(|(shard_id, _)| *shard_id)
        .collect();

    if !state.renew_lease(executor_id, now, lease_ttl) {
        return Err(ShardManagerError::Internal(format!(
            "executor {executor_id} holds no lease right after it was found"
        )));
    }

    let pending = state.lease_grant_for(executor_id).ok_or_else(|| {
        ShardManagerError::Internal(format!(
            "executor {executor_id} holds no lease right after it was renewed"
        ))
    })?;
    Ok((
        state,
        RenewalDecision {
            pending,
            mismatched,
        },
    ))
}

pub(super) fn full_assignment_recipients(
    rebalance: &Rebalance,
    full_assignment_executors: &HashSet<ExecutorId>,
) -> BTreeSet<ExecutorId> {
    rebalance
        .get_assignments()
        .assignments
        .keys()
        .chain(rebalance.get_unassignments().unassignments.keys())
        .copied()
        .chain(full_assignment_executors.iter().copied())
        .collect()
}

pub(super) fn repair_failed_deliveries(
    failed_assignments: BTreeSet<ExecutorId>,
    failed_unassignments: BTreeSet<ExecutorId>,
) -> Option<BTreeSet<ExecutorId>> {
    let unreached: BTreeSet<ExecutorId> = failed_unassignments
        .into_iter()
        .chain(failed_assignments)
        .collect();
    (!unreached.is_empty()).then_some(unreached)
}
