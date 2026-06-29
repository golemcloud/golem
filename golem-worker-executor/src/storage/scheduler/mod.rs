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

pub mod memory;
pub mod postgres;
pub mod sqlite;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::SafeDisplay;
use golem_common::model::{ScheduleId, ScheduledAction, ShardAssignment, ShardId};
use golem_service_base::repo::RepoError;
use std::fmt::{self, Debug, Display, Formatter};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedScheduledAction {
    pub schedule_id: ScheduleId,
    pub action: ScheduledAction,
    pub due_at: DateTime<Utc>,
    pub lease_owner: Uuid,
    pub attempt_count: u32,
}

/// Typed error for [`SchedulerStorage`] operations.
///
/// `Transient` errors (pool exhaustion, connection resets) are retriable;
/// `Other` errors (data issues, schema problems) are not.
#[derive(Debug, Clone)]
pub enum SchedulerStorageError {
    /// Transient error — pool exhaustion, connection reset, broken pipe.
    /// Caller may retry.
    Transient(String),
    /// Permanent error — data issue, unique violation, schema error.
    /// Caller should not retry.
    Other(String),
}

impl SchedulerStorageError {
    pub fn is_retriable(&self) -> bool {
        matches!(self, SchedulerStorageError::Transient(_))
    }
}

impl Display for SchedulerStorageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SchedulerStorageError::Transient(msg) => write!(f, "Transient storage error: {msg}"),
            SchedulerStorageError::Other(msg) => write!(f, "Storage error: {msg}"),
        }
    }
}

impl std::error::Error for SchedulerStorageError {}

impl From<String> for SchedulerStorageError {
    fn from(s: String) -> Self {
        SchedulerStorageError::Other(s)
    }
}

impl From<RepoError> for SchedulerStorageError {
    fn from(err: RepoError) -> Self {
        if err.is_transient() {
            SchedulerStorageError::Transient(err.to_safe_string())
        } else {
            SchedulerStorageError::Other(err.to_safe_string())
        }
    }
}

impl From<SchedulerStorageError> for String {
    fn from(err: SchedulerStorageError) -> Self {
        err.to_string()
    }
}

#[async_trait]
pub trait SchedulerStorage: Debug {
    async fn insert(
        &self,
        schedule_id: ScheduleId,
        due_at: DateTime<Utc>,
        shard_id: ShardId,
        action: &ScheduledAction,
    ) -> Result<(), SchedulerStorageError>;

    async fn cancel(&self, schedule_id: &ScheduleId) -> Result<(), SchedulerStorageError>;

    async fn claim_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
        limit: u32,
        lease_ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError>;

    async fn extend_lease(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, SchedulerStorageError>;

    async fn ack(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
    ) -> Result<bool, SchedulerStorageError>;
}

pub fn datetime_to_millis(time: DateTime<Utc>) -> i64 {
    time.timestamp_millis()
}

pub fn millis_to_datetime(millis: i64) -> Result<DateTime<Utc>, String> {
    DateTime::<Utc>::from_timestamp_millis(millis)
        .ok_or_else(|| format!("Invalid timestamp milliseconds: {millis}"))
}
