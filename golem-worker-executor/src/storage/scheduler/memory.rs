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

use super::{
    ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError, UnreadableClaims,
    UnreadableScheduledAction, datetime_to_millis, millis_to_datetime,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::{ScheduleId, ShardAssignment, ShardId};
use golem_common::serialization::decode;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Default)]
pub struct InMemorySchedulerStorage {
    entries: Mutex<HashMap<Uuid, ScheduledEntry>>,
}

#[derive(Debug, Clone)]
struct ScheduledEntry {
    due_at_ms: i64,
    shard_id: ShardId,
    action: Vec<u8>,
    lease_owner: Option<Uuid>,
    lease_until_ms: Option<i64>,
    attempt_count: u32,
}

impl InMemorySchedulerStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerStorage for InMemorySchedulerStorage {
    async fn insert(
        &self,
        schedule_id: ScheduleId,
        due_at: DateTime<Utc>,
        shard_id: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        self.entries
            .lock()
            .unwrap()
            .entry(schedule_id.id)
            .or_insert_with(|| ScheduledEntry {
                due_at_ms: datetime_to_millis(due_at),
                shard_id,
                action: action.to_vec(),
                lease_owner: None,
                lease_until_ms: None,
                attempt_count: 0,
            });
        Ok(())
    }

    async fn cancel(&self, schedule_id: &ScheduleId) -> Result<(), SchedulerStorageError> {
        self.entries.lock().unwrap().remove(&schedule_id.id);
        Ok(())
    }

    async fn claim_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
        limit: u32,
        lease_ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        let now_ms = datetime_to_millis(now);
        let lease_until = datetime_to_millis(now + lease_ttl);
        let lease_owner = Uuid::now_v7();

        let mut entries = self.entries.lock().unwrap();

        let mut candidates: Vec<(Uuid, i64)> = entries
            .iter()
            .filter(|(_, entry)| {
                entry.due_at_ms <= now_ms
                    && entry
                        .lease_until_ms
                        .is_none_or(|lease_until| lease_until <= now_ms)
                    && assignment.contains(&entry.shard_id)
            })
            .map(|(id, entry)| (*id, entry.due_at_ms))
            .collect();

        candidates.sort_by_key(|(id, due_at_ms)| (*due_at_ms, *id));

        let rows = candidates
            .into_iter()
            .take(limit as usize)
            .filter_map(|(id, _)| {
                let entry = entries.get_mut(&id)?;
                entry.lease_owner = Some(lease_owner);
                entry.lease_until_ms = Some(lease_until);
                entry.attempt_count += 1;
                let schedule_id = ScheduleId { id };
                Some(millis_to_datetime(entry.due_at_ms).map(|due_at| {
                    match decode(&entry.action) {
                        Ok(action) => Ok(ClaimedScheduledAction {
                            schedule_id,
                            action,
                            due_at,
                            lease_owner,
                            attempt_count: entry.attempt_count,
                        }),
                        Err(error) => Err(UnreadableScheduledAction { schedule_id, error }),
                    }
                }))
            })
            .collect::<Result<Vec<_>, String>>()?;
        drop(entries);

        UnreadableClaims::of_rows(rows)
    }

    async fn count_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError> {
        let now_ms = datetime_to_millis(now);
        Ok(self
            .entries
            .lock()
            .unwrap()
            .values()
            .filter(|entry| entry.due_at_ms <= now_ms && assignment.contains(&entry.shard_id))
            .count() as u64)
    }

    async fn extend_lease(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        lease_until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        let now_ms = datetime_to_millis(now);
        let requested_until_ms = datetime_to_millis(lease_until);
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get_mut(&schedule_id.id)
            && entry.lease_owner == Some(lease_owner)
            && let Some(current_until_ms) = entry.lease_until_ms
            && current_until_ms > now_ms
            && requested_until_ms > now_ms
        {
            entry.lease_until_ms = Some(current_until_ms.max(requested_until_ms));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn ack(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        now: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        let mut entries = self.entries.lock().unwrap();
        if entries.get(&schedule_id.id).is_some_and(|entry| {
            entry.lease_owner == Some(lease_owner)
                && entry
                    .lease_until_ms
                    .is_some_and(|until| until > datetime_to_millis(now))
        }) {
            entries.remove(&schedule_id.id);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    async fn claim_due_names_the_schedule_with_an_unreadable_action() {
        let storage = InMemorySchedulerStorage::default();
        let now = Utc::now();
        let schedule_id = ScheduleId::fresh();
        let shard_id = ShardId::new(3);
        storage
            .insert(schedule_id, now, shard_id, b"\xffprivate payload")
            .await
            .unwrap();

        let error = storage
            .claim_due(
                now,
                &ShardAssignment::unexpiring(4, [shard_id]),
                1,
                Duration::from_secs(30),
            )
            .await
            .unwrap_err();

        let SchedulerStorageError::Unreadable(claims) = &error else {
            panic!("the failed decode must be an unreadable action: {error}");
        };
        assert_eq!(
            **claims,
            UnreadableClaims {
                claimed: Vec::new(),
                unreadable: vec![UnreadableScheduledAction {
                    schedule_id,
                    error: golem_common::serialization::DecodeError::UnknownVersion(255),
                }],
            }
        );
        let text = error.to_string();
        assert!(
            text.contains(&format!(
                "failed to decode scheduled action {schedule_id}: unknown serialization version: 255"
            )),
            "{text}"
        );
        assert!(!text.contains("private payload"), "{text}");
    }

    /// A batch with an action that does not decode returns the healthy claims with the error of
    /// the bad row. The bad row is not acknowledged, so it stays stored.
    #[test]
    async fn claim_due_returns_the_healthy_claims_with_the_unreadable_row() {
        let storage = InMemorySchedulerStorage::default();
        let now = Utc::now();
        let shard_id = ShardId::new(5);
        let assignment = ShardAssignment::unexpiring(8, [shard_id]);
        let action = golem_common::model::ScheduledAction::Resume {
            agent_created_by: golem_common::model::account::AccountId::new(),
            owned_agent_id: golem_common::model::OwnedAgentId::new(
                golem_common::model::environment::EnvironmentId::new(),
                &crate::test_agent_id("scheduler-mixed-batch"),
            ),
            target_worker_fingerprint: golem_common::model::AgentFingerprint::new(),
        };
        let first = ScheduleId::fresh();
        let bad = ScheduleId::fresh();
        let last = ScheduleId::fresh();
        let encoded = golem_common::serialization::serialize(&action).unwrap();
        storage
            .insert(
                first,
                now - chrono::Duration::seconds(3),
                shard_id,
                &encoded,
            )
            .await
            .unwrap();
        storage
            .insert(
                bad,
                now - chrono::Duration::seconds(2),
                shard_id,
                b"\xfeprivate payload",
            )
            .await
            .unwrap();
        storage
            .insert(last, now - chrono::Duration::seconds(1), shard_id, &encoded)
            .await
            .unwrap();

        let error = storage
            .claim_due(now, &assignment, 3, Duration::from_secs(30))
            .await
            .unwrap_err();

        let SchedulerStorageError::Unreadable(claims) = &error else {
            panic!("a bad row must be an unreadable claim: {error}");
        };
        assert_eq!(
            claims
                .claimed
                .iter()
                .map(|claim| (claim.schedule_id, claim.action.clone(), claim.attempt_count))
                .collect::<Vec<_>>(),
            vec![(first, action.clone(), 1), (last, action, 1)]
        );
        assert_eq!(
            claims.unreadable,
            vec![UnreadableScheduledAction {
                schedule_id: bad,
                error: golem_common::serialization::DecodeError::UnknownVersion(254),
            }]
        );
        assert!(!error.to_string().contains("private payload"), "{error}");
        assert_eq!(storage.count_due(now, &assignment).await.unwrap(), 3);
    }
}
