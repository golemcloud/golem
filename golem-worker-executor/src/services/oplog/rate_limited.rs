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

use crate::metrics::oplog::record_oplog_rate_limited;
use crate::model::ExecutionStatus;
use crate::services::oplog::{CommitLevel, Oplog, OplogService};
use crate::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use golem_common::model::account::AccountId;
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

fn make_limiter(per_second: u32) -> DefaultDirectRateLimiter {
    let quota = Quota::per_second(NonZeroU32::new(per_second).unwrap_or(nonzero!(1u32)));
    RateLimiter::direct(quota)
}

/// A wrapper around an [`Oplog`] that rate-limits calls to [`Oplog::add`].
///
/// Each agent (worker) receives its own `RateLimitedOplog` with an independent
/// token bucket, so the limit is enforced **per agent** — analogous to
/// `max_memory_per_worker` or `per_invocation_http_call_limit`.
///
/// The rate limit (writes per second) is read dynamically from the supplied
/// [`AtomicResourceEntry`] on every `add` call. When the value changes the governor
/// token bucket is rebuilt atomically behind an [`ArcSwap`] so the hot path
/// remains lock-free. When the entry reports
/// [`AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND`] the governor is bypassed entirely.
///
/// All other [`Oplog`] methods are pure delegation to the inner oplog.
pub struct RateLimitedOplog {
    inner: Arc<dyn Oplog>,
    resource_entry: Arc<AtomicResourceEntry>,
    /// Current governor rate limiter. Swapped atomically when the rate changes.
    limiter: ArcSwap<DefaultDirectRateLimiter>,
    /// Last rate value used to build the current limiter. Used to detect changes.
    cached_rate: AtomicU64,
}

impl RateLimitedOplog {
    pub fn new(inner: Arc<dyn Oplog>, resource_entry: Arc<AtomicResourceEntry>) -> Self {
        let initial_rate = resource_entry.oplog_writes_per_second();
        let initial_limiter = limiter_for_rate(initial_rate);
        Self {
            inner,
            resource_entry,
            limiter: ArcSwap::from(initial_limiter),
            cached_rate: AtomicU64::new(initial_rate),
        }
    }
}

/// Returns an `Arc<DefaultDirectRateLimiter>` for the given rate, or a placeholder
/// limiter that is never consulted when the rate is
/// [`AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND`].
fn limiter_for_rate(rate: u64) -> Arc<DefaultDirectRateLimiter> {
    if rate >= AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND || rate == 0 {
        // Placeholder — never consulted for unlimited rates.
        Arc::new(make_limiter(1))
    } else {
        let clamped = rate.min(u32::MAX as u64) as u32;
        Arc::new(make_limiter(clamped))
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
        let rate = self.resource_entry.oplog_writes_per_second();

        if rate > 0 && rate < AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND {
            // Detect rate change and atomically swap in a new limiter if needed.
            // compare_exchange ensures only one concurrent caller rebuilds
            // the limiter; losers simply proceed with the current one.
            let cached = self.cached_rate.load(Ordering::Acquire);
            if cached != rate
                && self
                    .cached_rate
                    .compare_exchange(cached, rate, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                self.limiter.store(limiter_for_rate(rate));
            }

            let limiter = self.limiter.load();
            // until_ready returns immediately when a token is available,
            // otherwise sleeps until one is. We only log/record the metric
            // when we actually had to wait.
            if limiter.check().is_err() {
                debug!("RateLimitedOplog: back-pressure applied (rate={rate} writes/sec)");
                record_oplog_rate_limited();
            }
            limiter.until_ready().await;
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
/// [`RateLimitedOplog`] that holds the per-account [`AtomicResourceEntry`] resolved from
/// [`ResourceLimits`]. The entry is shared with the background sync loop that refreshes plan
/// limits every ~60 seconds, so rate limit changes take effect automatically without reopening
/// the oplog. All other service methods are pure delegation.
pub struct RateLimitedOplogService {
    inner: Arc<dyn OplogService>,
    resource_limits: Arc<dyn ResourceLimits>,
}

impl RateLimitedOplogService {
    pub fn new(inner: Arc<dyn OplogService>, resource_limits: Arc<dyn ResourceLimits>) -> Self {
        Self {
            inner,
            resource_limits,
        }
    }

    async fn entry_for(&self, account_id: AccountId) -> Arc<AtomicResourceEntry> {
        self.resource_limits
            .initialize_account(account_id)
            .await
            .unwrap_or_else(|_| {
                // On registry error fall back to unlimited so a transient outage
                // never blocks oplog writes entirely.
                Arc::new(AtomicResourceEntry::new(
                    u64::MAX,
                    usize::MAX,
                    usize::MAX,
                    u64::MAX,
                    AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                ))
            })
    }
}

impl std::fmt::Debug for RateLimitedOplogService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimitedOplogService").finish()
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
        let resource_entry = self.entry_for(initial_worker_metadata.created_by).await;
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
        Arc::new(RateLimitedOplog::new(inner_oplog, resource_entry))
    }

    async fn open(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog> {
        let resource_entry = self.entry_for(initial_worker_metadata.created_by).await;
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
        Arc::new(RateLimitedOplog::new(inner_oplog, resource_entry))
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionStatus;
    use crate::services::oplog::PrimaryOplogService;
    use crate::services::resource_limits::ResourceLimits;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use async_trait::async_trait;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::regions::OplogRegion;
    use golem_common::model::{AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, Timestamp};
    use golem_common::read_only_lock;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::sync::RwLock;
    use std::time::Instant;
    use test_r::test;
    use uuid::Uuid;

    test_r::enable!();

    /// [`ResourceLimits`] stub that always returns the same pre-seeded entry
    /// regardless of account. Allows tests to inject a specific rate directly.
    struct FixedResourceLimits {
        entry: Arc<AtomicResourceEntry>,
    }

    #[async_trait]
    impl ResourceLimits for FixedResourceLimits {
        async fn initialize_account(
            &self,
            _account_id: AccountId,
        ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
            Ok(self.entry.clone())
        }
    }

    fn make_entry(writes_per_second: u64) -> Arc<AtomicResourceEntry> {
        let entry = Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        ));
        entry.set_oplog_writes_per_second(writes_per_second);
        entry
    }

    fn resource_limits_with_rate(writes_per_second: u64) -> Arc<dyn ResourceLimits> {
        Arc::new(FixedResourceLimits {
            entry: make_entry(writes_per_second),
        })
    }

    fn resource_limits_with_entry(entry: Arc<AtomicResourceEntry>) -> Arc<dyn ResourceLimits> {
        Arc::new(FixedResourceLimits { entry })
    }

    async fn make_oplog(resource_limits: Arc<dyn ResourceLimits>) -> Arc<dyn Oplog> {
        let indexed = Arc::new(InMemoryIndexedStorage::new());
        let blob = Arc::new(InMemoryBlobStorage::new());
        let service = RateLimitedOplogService::new(
            Arc::new(PrimaryOplogService::new(indexed, blob, 1, 1, 4096).await),
            resource_limits,
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
        let oplog = make_oplog(resource_limits_with_rate(5)).await;

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

    // When the rate is UNLIMITED_OPLOG_WRITES_PER_SECOND, writes complete with
    // no meaningful delay — well under 100 ms for 100 entries.
    #[test]
    async fn unlimited_rate_has_no_meaningful_delay() {
        let oplog = make_oplog(resource_limits_with_rate(
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ))
        .await;

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

    // A rate of 0 is treated as unlimited (same as the proto3 default
    // normalisation) — writes must complete with no meaningful delay.
    #[test]
    async fn zero_rate_is_treated_as_unlimited() {
        let oplog = make_oplog(resource_limits_with_rate(0)).await;

        let start = Instant::now();
        for _ in 0..100 {
            oplog.add(dummy_entry()).await;
        }
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "Expected rate=0 (unlimited) writes to complete in under 100ms, got {elapsed:?}"
        );
    }

    // Lowering the rate at runtime (unlimited -> 5/sec) takes effect on
    // subsequent `add` calls: the governor is rebuilt and back-pressure kicks in.
    #[test]
    async fn dynamic_rate_change_from_unlimited_to_limited() {
        let entry = make_entry(AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND);
        let oplog = make_oplog(resource_limits_with_entry(entry.clone())).await;

        // Unlimited — should be fast.
        let start = Instant::now();
        for _ in 0..20 {
            oplog.add(dummy_entry()).await;
        }
        let fast_elapsed = start.elapsed();
        assert!(
            fast_elapsed < Duration::from_millis(100),
            "Expected unlimited writes to be fast, got {fast_elapsed:?}"
        );

        // Switch to 5/sec at runtime.
        entry.set_oplog_writes_per_second(5);

        // 15 writes at 5/sec (burst=5) must take >= 1.5 s.
        let start = Instant::now();
        for _ in 0..15 {
            oplog.add(dummy_entry()).await;
        }
        let slow_elapsed = start.elapsed();
        assert!(
            slow_elapsed >= Duration::from_millis(1500),
            "Expected at least 1.5s after lowering rate to 5/sec, got {slow_elapsed:?}"
        );
    }

    // Raising the rate at runtime (5/sec -> unlimited) takes effect on
    // subsequent `add` calls: the governor is bypassed and writes fly through.
    #[test]
    async fn dynamic_rate_change_from_limited_to_unlimited() {
        let entry = make_entry(5);
        let oplog = make_oplog(resource_limits_with_entry(entry.clone())).await;

        // 15 writes at 5/sec — must be slow.
        let start = Instant::now();
        for _ in 0..15 {
            oplog.add(dummy_entry()).await;
        }
        let slow_elapsed = start.elapsed();
        assert!(
            slow_elapsed >= Duration::from_millis(1500),
            "Expected at least 1.5s at 5/sec, got {slow_elapsed:?}"
        );

        // Switch to unlimited at runtime.
        entry.set_oplog_writes_per_second(
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        );

        // 100 writes at unlimited — should be fast.
        let start = Instant::now();
        for _ in 0..100 {
            oplog.add(dummy_entry()).await;
        }
        let fast_elapsed = start.elapsed();
        assert!(
            fast_elapsed < Duration::from_millis(100),
            "Expected unlimited writes to be fast after raising rate, got {fast_elapsed:?}"
        );
    }
}
