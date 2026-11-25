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

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::{ScheduleId, ScheduledAction, ShardAssignment, ShardId};
use golem_worker_executor::storage::scheduler::{
    ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Debug, Default)]
pub(super) struct RetentionInsertGate {
    pub hold_next: AtomicBool,
    pub arrived: Notify,
    pub rescheduled: Notify,
}

/// Leaves one retention insertion pending so the executor can be lost after its terminal commit.
#[derive(Debug)]
pub(super) struct HoldRetentionInsert {
    pub inner: Arc<dyn SchedulerStorage + Send + Sync>,
    pub gate: Arc<RetentionInsertGate>,
}

#[async_trait]
impl SchedulerStorage for HoldRetentionInsert {
    async fn insert(
        &self,
        id: ScheduleId,
        due: DateTime<Utc>,
        shard: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        if matches!(
            golem_common::serialization::deserialize::<ScheduledAction>(action).unwrap(),
            ScheduledAction::DeleteFinishedInvocation { .. }
        ) && self.gate.hold_next.swap(false, Ordering::SeqCst)
        {
            self.gate.arrived.notify_one();
            futures::future::pending::<()>().await;
        }
        self.inner.insert(id, due, shard, action).await
    }

    async fn cancel(&self, id: &ScheduleId) -> Result<(), SchedulerStorageError> {
        self.inner.cancel(id).await
    }

    async fn get_action(&self, id: &ScheduleId) -> Result<Option<Vec<u8>>, SchedulerStorageError> {
        self.inner.get_action(id).await
    }

    async fn replace_action(
        &self,
        id: &ScheduleId,
        expected: &[u8],
        action: &[u8],
        due: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.replace_action(id, expected, action, due).await
    }

    async fn claim_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
        limit: u32,
        ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        self.inner.claim_due(now, assignment, limit, ttl).await
    }

    async fn count_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError> {
        self.inner.count_due(now, assignment).await
    }

    async fn extend_lease(
        &self,
        id: &ScheduleId,
        owner: Uuid,
        until: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.extend_lease(id, owner, until, now).await
    }

    async fn reschedule(
        &self,
        id: &ScheduleId,
        owner: Uuid,
        due: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        let moved = self.inner.reschedule(id, owner, due, now).await?;
        if moved {
            self.gate.rescheduled.notify_one();
        }
        Ok(moved)
    }

    async fn ack(
        &self,
        id: &ScheduleId,
        owner: Uuid,
        now: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.ack(id, owner, now).await
    }
}
