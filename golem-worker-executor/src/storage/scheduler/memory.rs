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
    ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError, datetime_to_millis,
    millis_to_datetime,
};
use async_trait::async_trait;

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
        due_at: jiff::Timestamp,
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

    async fn get_action(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Option<Vec<u8>>, SchedulerStorageError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(&schedule_id.id)
            .map(|entry| entry.action.clone()))
    }

    async fn replace_action(
        &self,
        schedule_id: &ScheduleId,
        expected_action: &[u8],
        action: &[u8],
        due_at: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get_mut(&schedule_id.id)
            && entry.action == expected_action
        {
            entry.action = action.to_vec();
            entry.due_at_ms = datetime_to_millis(due_at);
            entry.lease_owner = None;
            entry.lease_until_ms = None;
            entry.attempt_count = 0;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn claim_due(
        &self,
        now: jiff::Timestamp,
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

        let mut result = Vec::new();
        for (id, _) in candidates.into_iter().take(limit as usize) {
            if let Some(entry) = entries.get_mut(&id) {
                entry.lease_owner = Some(lease_owner);
                entry.lease_until_ms = Some(lease_until);
                entry.attempt_count += 1;
                result.push(ClaimedScheduledAction {
                    schedule_id: ScheduleId { id },
                    action: decode(&entry.action).map_err(|error| error.to_string())?,
                    due_at: millis_to_datetime(entry.due_at_ms)?,
                    lease_owner,
                    attempt_count: entry.attempt_count,
                });
            }
        }

        Ok(result)
    }

    async fn count_due(
        &self,
        now: jiff::Timestamp,
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
        lease_until: jiff::Timestamp,
        now: jiff::Timestamp,
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

    async fn reschedule(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        due_at: jiff::Timestamp,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get_mut(&schedule_id.id)
            && entry.lease_owner == Some(lease_owner)
            && entry
                .lease_until_ms
                .is_some_and(|until| until > datetime_to_millis(now))
        {
            entry.due_at_ms = datetime_to_millis(due_at);
            entry.lease_owner = None;
            entry.lease_until_ms = None;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn ack(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        now: jiff::Timestamp,
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
