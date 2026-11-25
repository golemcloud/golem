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
    pub due_at: jiff::Timestamp,
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
        due_at: jiff::Timestamp,
        shard_id: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError>;

    async fn cancel(&self, schedule_id: &ScheduleId) -> Result<(), SchedulerStorageError>;

    async fn claim_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
        limit: u32,
        lease_ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError>;

    /// Counts actions whose due time has passed and have not been acknowledged,
    /// including actions currently leased by an in-flight scheduler tick.
    async fn count_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError>;

    /// Renews a matching lease that is still live as of `now`, without shortening it.
    /// Times are compared at millisecond precision; equality with expiry is expired.
    /// Returns false for a lost lease or a requested deadline not later than `now`.
    /// As with `claim_due`, `now` is trusted caller time, not database execution time:
    /// a wait after sampling it does not advance the authorization check.
    async fn extend_lease(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        lease_until: jiff::Timestamp,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError>;

    /// Deletes only a matching lease that is live as of the caller-supplied `now`.
    /// Uses the same millisecond, as-of time semantics as `extend_lease`.
    async fn ack(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError>;
}

pub fn datetime_to_millis(time: jiff::Timestamp) -> i64 {
    time.as_nanosecond().div_euclid(1_000_000) as i64
}

pub fn millis_to_datetime(millis: i64) -> Result<jiff::Timestamp, String> {
    jiff::Timestamp::from_millisecond(millis)
        .map_err(|_| format!("Invalid timestamp milliseconds: {millis}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn scheduler_milliseconds_floor_before_the_epoch() {
        let timestamp = jiff::Timestamp::from_nanosecond(-1).unwrap();
        assert_eq!(datetime_to_millis(timestamp), -1);
        assert_eq!(millis_to_datetime(-1).unwrap().as_nanosecond(), -1_000_000);
        assert!(millis_to_datetime(i64::MAX).is_err());
    }
}
