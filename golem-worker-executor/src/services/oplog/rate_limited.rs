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

use crate::model::ExecutionStatus;
use crate::services::oplog::{CommitLevel, Oplog, OplogService};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, PayloadId, PersistenceLevel, RawOplogPayload,
};
use golem_common::model::{AgentMetadata, AgentStatusRecord, OwnedAgentId, ScanCursor};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use nonzero_ext::nonzero;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::debug;

/// Sentinel value meaning "no rate limit"
pub const UNLIMITED: u64 = u64::MAX;

fn make_limiter(per_second: u32) -> DefaultDirectRateLimiter {
    let quota = Quota::per_second(NonZeroU32::new(per_second).unwrap_or(nonzero!(1u32)));
    RateLimiter::direct(quota)
}

/// A wrapper around an [`Oplog`] that rate-limits calls to [`Oplog::add`].
///
/// When the configured rate (in writes per second) is [`UNLIMITED`], calls pass through
/// immediately with no governor overhead. When a limit is set, `add` async-sleeps until
/// the governor token bucket allows the write, then delegates to the inner oplog.
///
/// All other [`Oplog`] methods are pure delegation to the inner oplog.
pub struct RateLimitedOplog {
    inner: Arc<dyn Oplog>,
    /// Current governor rate limiter. Swapped atomically when the rate changes.
    limiter: ArcSwap<DefaultDirectRateLimiter>,
    /// Last rate value used to build the current limiter. Used to detect changes.
    cached_rate: AtomicU64,
}

impl RateLimitedOplog {
    /// Creates a new `RateLimitedOplog` wrapping `inner`.
    ///
    /// `writes_per_second` sets the initial rate limit.  Pass [`UNLIMITED`] to disable
    /// rate limiting entirely (the `ArcSwap` is still allocated but never consulted).
    pub fn new(inner: Arc<dyn Oplog>, writes_per_second: u64) -> Self {
        // Clamp to u32::MAX for governor; anything that large is effectively unlimited anyway.
        let clamped = writes_per_second.min(u32::MAX as u64) as u32;
        let initial_limiter = if writes_per_second == UNLIMITED || clamped == 0 {
            // We need something in the ArcSwap even when unlimited; it is never consulted.
            Arc::new(make_limiter(1))
        } else {
            Arc::new(make_limiter(clamped))
        };
        Self {
            inner,
            limiter: ArcSwap::from(initial_limiter),
            cached_rate: AtomicU64::new(writes_per_second),
        }
    }

    /// Returns the currently configured rate limit (writes per second), or [`UNLIMITED`].
    pub fn current_rate(&self) -> u64 {
        self.cached_rate.load(Ordering::Acquire)
    }

    /// Updates the rate limit. If `new_rate` differs from the current cached rate, the
    /// governor is rebuilt and the `ArcSwap` is updated atomically.
    pub fn set_rate(&self, new_rate: u64) {
        let old = self.cached_rate.load(Ordering::Acquire);
        if old == new_rate {
            return;
        }
        self.cached_rate.store(new_rate, Ordering::Release);
        if new_rate != UNLIMITED {
            let clamped = new_rate.min(u32::MAX as u64) as u32;
            if clamped > 0 {
                self.limiter.store(Arc::new(make_limiter(clamped)));
            }
        }
    }
}

impl Debug for RateLimitedOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimitedOplog")
            .field("rate", &self.cached_rate.load(Ordering::Relaxed))
            .finish()
    }
}

#[async_trait]
impl Oplog for RateLimitedOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        let rate = self.cached_rate.load(Ordering::Acquire);
        if rate != UNLIMITED {
            let limiter = self.limiter.load();
            if limiter.check().is_err() {
                debug!("RateLimitedOplog: back-pressure applied (rate={rate} writes/sec)");
                limiter.until_ready().await;
            }
        }
        self.inner.add(entry).await
    }

    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
        self.inner.drop_prefix(last_dropped_id).await
    }

    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.commit(level).await
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        self.inner.current_oplog_index().await
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        self.inner.last_added_non_hint_entry().await
    }

    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool {
        self.inner.wait_for_replicas(replicas, timeout).await
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        self.inner.read(oplog_index).await
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read_many(oplog_index, n).await
    }

    async fn length(&self) -> u64 {
        self.inner.length().await
    }

    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(data).await
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner.download_raw_payload(payload_id, md5_hash).await
    }

    async fn switch_persistence_level(&self, mode: PersistenceLevel) {
        self.inner.switch_persistence_level(mode).await;
    }

    fn inner(&self) -> Option<Arc<dyn Oplog>> {
        Some(self.inner.clone())
    }
}

/// A thin [`OplogService`] wrapper that rate-limits [`Oplog::add`] calls on every oplog
/// instance it creates or opens.
///
/// `create` and `open` delegate to the inner service, then wrap the returned oplog in a
/// [`RateLimitedOplog`]. All other service methods are pure delegation.
#[derive(Debug)]
pub struct RateLimitedOplogService {
    inner: Arc<dyn OplogService>,
    writes_per_second: u64,
}

impl RateLimitedOplogService {
    pub fn new(inner: Arc<dyn OplogService>, writes_per_second: u64) -> Self {
        Self {
            inner,
            writes_per_second,
        }
    }
}

#[async_trait]
impl OplogService for RateLimitedOplogService {
    async fn create(
        &self,
        owned_agent_id: &OwnedAgentId,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        let inner_oplog = self
            .inner
            .create(
                owned_agent_id,
                initial_entry,
                initial_worker_metadata,
                last_known_status,
                execution_status,
            )
            .await;
        Arc::new(RateLimitedOplog::new(inner_oplog, self.writes_per_second))
    }

    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        let inner_oplog = self
            .inner
            .open(
                owned_agent_id,
                last_oplog_index,
                initial_worker_metadata,
                last_known_status,
                execution_status,
            )
            .await;
        Arc::new(RateLimitedOplog::new(inner_oplog, self.writes_per_second))
    }

    async fn get_last_index(&self, owned_agent_id: &OwnedAgentId) -> OplogIndex {
        self.inner.get_last_index(owned_agent_id).await
    }

    async fn delete(&self, owned_agent_id: &OwnedAgentId) {
        self.inner.delete(owned_agent_id).await
    }

    async fn read(
        &self,
        owned_agent_id: &OwnedAgentId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read(owned_agent_id, idx, n).await
    }

    async fn exists(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.inner.exists(owned_agent_id).await
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(environment_id, component_id, cursor, count)
            .await
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(owned_agent_id, data).await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner
            .download_raw_payload(owned_agent_id, payload_id, md5_hash)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionStatus;
    use crate::services::oplog::PrimaryOplogService;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::regions::OplogRegion;
    use golem_common::model::{AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, Timestamp};
    use golem_common::read_only_lock;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::sync::RwLock;
    use std::time::Instant;
    use test_r::test;
    use uuid::Uuid;

    test_r::enable!();

    async fn make_oplog(writes_per_second: u64) -> Arc<dyn Oplog> {
        let indexed = Arc::new(InMemoryIndexedStorage::new());
        let blob = Arc::new(InMemoryBlobStorage::new());
        let service = RateLimitedOplogService::new(
            Arc::new(PrimaryOplogService::new(indexed, blob, 1, 1, 4096).await),
            writes_per_second,
        );

        let account_id = AccountId::new();
        let env_id = EnvironmentId::new();
        let agent_id = AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "test".to_string(),
        };
        let owned = OwnedAgentId::new(env_id, &agent_id);

        let last_known_status = read_only_lock::tokio::ReadOnlyLock::new(Arc::new(
            tokio::sync::RwLock::new(AgentStatusRecord::default()),
        ));
        let execution_status = read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
            ExecutionStatus::Suspended {
                agent_mode: AgentMode::Durable,
                timestamp: Timestamp::now_utc(),
            },
        )));

        service
            .open(
                &owned,
                None,
                AgentMetadata::default(agent_id, account_id, env_id),
                last_known_status,
                execution_status,
            )
            .await
    }

    fn dummy_entry() -> OplogEntry {
        OplogEntry::jump(OplogRegion {
            start: OplogIndex::from_u64(1),
            end: OplogIndex::from_u64(1),
        })
    }

    // When writes exceed the configured rate, subsequent adds are delayed.
    // With a 5/sec rate and a burst of 15 writes, the governor will exhaust the
    // initial burst of 5 immediately, then must wait ~2 more seconds to emit
    // the remaining 10. Total elapsed must be >= 1.5 s (conservative bound).
    #[test]
    async fn rate_limit_slows_down_writes_that_exceed_the_quota() {
        let oplog = make_oplog(5).await;

        let start = Instant::now();
        for _ in 0..15 {
            oplog.add(dummy_entry()).await;
        }
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(1500),
            "Expected at least 1.5s for 15 writes at 5/sec (burst=5), got {elapsed:?}"
        );
    }

    // When the rate is UNLIMITED (u64::MAX), writes complete with no meaningful
    // delay — well under 100 ms for 100 entries.
    #[test]
    async fn unlimited_rate_has_no_meaningful_delay() {
        let oplog = make_oplog(UNLIMITED).await;

        let start = Instant::now();
        for _ in 0..100 {
            oplog.add(dummy_entry()).await;
        }
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "Expected unlimited writes to complete in under 100ms, got {elapsed:?}"
        );
    }
}
