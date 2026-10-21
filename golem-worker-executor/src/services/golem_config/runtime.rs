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

use golem_common::SafeDisplay;
use golem_common::config::byte_size;
use golem_common::model::{
    DEFAULT_INVOCATION_RESULT_BLOOM_BITS, DEFAULT_INVOCATION_RESULT_BLOOM_HASHES,
    DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY, InvocationResultMembership,
};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::num::NonZeroUsize;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub max_active_agents: usize,
    pub invocation_result_broadcast_capacity: usize,
    pub live_stream_event_broadcast_capacity: NonZeroUsize,
    pub max_concurrent_streams: u32,
    pub event_broadcast_capacity: usize,
    pub event_history_size: usize,
    pub fuel_to_borrow: u64,
    pub ephemeral_fuel_overdraft_multiplier: u64,
    #[serde(with = "humantime_serde")]
    pub epoch_interval: Duration,
    pub epoch_ticks: u64,
    pub max_oplog_query_pages_size: usize,
    /// Maximum encoded size of one oplog query page. A page always holds at least one entry.
    pub max_oplog_query_page_bytes: usize,
    /// Oplog query pages omit entries whose encoded size is larger than this. The oplog entry
    /// read returns them in full.
    pub max_inline_oplog_entry_size: usize,
    /// Maximum number of oplog entry reads that hold a full entry in memory at the same time.
    /// Further reads wait for a running read to end.
    pub max_concurrent_oplog_entry_reads: usize,
    pub max_invocation_context_stack_depth: usize,
    /// Maximum accepted payload bytes in each completion-buffered tool attachment direction.
    pub max_tool_attachment_bytes: usize,
    /// Optional wall-clock upper bound for a single guest invocation. When exceeded, the
    /// invocation fails like a trap (no invocation-finished marker is written) and normal
    /// retry handling applies. `None` (the default) leaves invocations unbounded.
    #[serde(with = "humantime_serde", default)]
    pub max_invocation_duration: Option<Duration>,
    /// Maximum time to drain active tail work after a guest export returns.
    #[serde(with = "humantime_serde")]
    pub tail_work_settle_timeout: Duration,
}

impl SafeDisplay for Limits {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "max active agents: {}", self.max_active_agents);
        let _ = writeln!(
            &mut result,
            "invocation result broadcast capacity: {}",
            self.invocation_result_broadcast_capacity
        );
        let _ = writeln!(
            &mut result,
            "live stream event broadcast capacity: {}",
            self.live_stream_event_broadcast_capacity
        );
        let _ = writeln!(
            &mut result,
            "max concurrent streams: {}",
            self.max_concurrent_streams
        );
        let _ = writeln!(
            &mut result,
            "event broadcast capacity: {}",
            self.event_broadcast_capacity
        );
        let _ = writeln!(
            &mut result,
            "event history size: {}",
            self.event_history_size
        );
        let _ = writeln!(&mut result, "fuel to borrow: {}", self.fuel_to_borrow);
        let _ = writeln!(
            &mut result,
            "ephemeral fuel overdraft multiplier: {}",
            self.ephemeral_fuel_overdraft_multiplier
        );
        let _ = writeln!(
            &mut result,
            "epoch interval: {}",
            self.epoch_interval.as_secs()
        );
        let _ = writeln!(&mut result, "epoch ticks: {}", self.epoch_ticks);
        let _ = writeln!(
            &mut result,
            "max oplog query pages: {}",
            self.max_oplog_query_pages_size
        );
        let _ = writeln!(
            &mut result,
            "max oplog query page bytes: {}",
            self.max_oplog_query_page_bytes
        );
        let _ = writeln!(
            &mut result,
            "max inline oplog entry size: {}",
            self.max_inline_oplog_entry_size
        );
        let _ = writeln!(
            &mut result,
            "max concurrent oplog entry reads: {}",
            self.max_concurrent_oplog_entry_reads
        );
        let _ = writeln!(
            &mut result,
            "max invocation context stack depth: {}",
            self.max_invocation_context_stack_depth
        );
        let _ = writeln!(
            &mut result,
            "max tool attachment bytes: {}",
            self.max_tool_attachment_bytes
        );
        let _ = writeln!(
            &mut result,
            "max invocation duration: {:?}",
            self.max_invocation_duration
        );
        let _ = writeln!(
            &mut result,
            "tail work settle timeout: {:?}",
            self.tail_work_settle_timeout
        );

        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DurableStreamConfig {
    /// Maximum encoded payload size of one external Durable Streams read or append.
    #[serde(with = "byte_size::required")]
    pub external_batch_max_size: usize,
    #[serde(with = "humantime_serde")]
    pub lease_ttl: Duration,
    /// Delay before retrying unfinished stream recovery, never a healthy-attachment poll.
    #[serde(with = "humantime_serde")]
    pub reconciliation_interval: Duration,
    pub reconciliation_batch_size: usize,
    #[serde(with = "humantime_serde")]
    pub abandoned_prepare_after: Duration,
}

impl DurableStreamConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.external_batch_max_size > 0,
            "external durable stream batch limit must be non-zero"
        );
        anyhow::ensure!(
            self.lease_ttl
                == Duration::from_millis(
                    golem_common::base_model::durable_stream::STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
                ),
            "durable stream lease TTL is fixed at 60 seconds in protocol v1"
        );
        anyhow::ensure!(
            self.abandoned_prepare_after
                == Duration::from_millis(
                    golem_common::base_model::durable_stream::STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS,
                ),
            "durable stream abandoned-prepare threshold is fixed at 5 minutes in protocol v1"
        );
        anyhow::ensure!(
            !self.reconciliation_interval.is_zero() && self.reconciliation_batch_size > 0,
            "durable stream reconciliation settings must be non-zero"
        );
        Ok(())
    }
}

impl Default for DurableStreamConfig {
    fn default() -> Self {
        Self {
            external_batch_max_size: 8 * 1024 * 1024,
            lease_ttl: Duration::from_millis(
                golem_common::base_model::durable_stream::STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
            ),
            reconciliation_interval: Duration::from_millis(
                golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RECONCILIATION_INTERVAL_MILLIS,
            ),
            reconciliation_batch_size:
                golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RECONCILIATION_BATCH_SIZE,
            abandoned_prepare_after: Duration::from_millis(
                golem_common::base_model::durable_stream::STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS,
            ),
        }
    }
}

impl SafeDisplay for DurableStreamConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "external batch maximum size: {}",
            humansize::ISizeFormatter::new(self.external_batch_max_size, humansize::BINARY)
        );
        let _ = writeln!(&mut result, "lease TTL: {:?}", self.lease_ttl);
        let _ = writeln!(
            &mut result,
            "recovery retry interval: {:?}",
            self.reconciliation_interval
        );
        let _ = writeln!(
            &mut result,
            "reconciliation batch size: {}",
            self.reconciliation_batch_size
        );
        let _ = writeln!(
            &mut result,
            "abandoned prepare after: {:?}",
            self.abandoned_prepare_after
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuspendConfig {
    #[serde(with = "humantime_serde")]
    pub suspend_after: Duration,
    #[serde(with = "humantime_serde")]
    pub ephemeral_max_sleep: Duration,
    #[serde(with = "humantime_serde")]
    pub wait_suspend_grace: Duration,
    #[serde(with = "humantime_serde")]
    pub wait_suspend_check_interval: Duration,
    #[serde(with = "humantime_serde")]
    pub rpc_suspend_after: Duration,
    #[serde(with = "humantime_serde")]
    pub rpc_resume_after: Duration,
}

impl SafeDisplay for SuspendConfig {
    fn to_safe_string(&self) -> String {
        format!(
            "suspend after: {:?}, ephemeral max sleep: {:?}, wait suspend grace: {:?}, wait suspend check interval: {:?}, RPC suspend after: {:?}, RPC resume after: {:?}",
            self.suspend_after,
            self.ephemeral_max_sleep,
            self.wait_suspend_grace,
            self.wait_suspend_check_interval,
            self.rpc_suspend_after,
            self.rpc_resume_after
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveAgentsConfig {
    pub drop_when_full: f64,
    #[serde(with = "humantime_serde")]
    pub ttl: Duration,
}

impl SafeDisplay for ActiveAgentsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "drop when full: {}", self.drop_when_full);
        let _ = writeln!(&mut result, "ttl: {:?}", self.ttl);
        result
    }
}

/// Controls how the cached `AgentStatusRecord` blob is written back to the KV store.
///
/// When `enabled`, the blob is no longer written on every oplog commit; instead the worker is
/// marked dirty and a single background sweeper coalesces the writes, flushing each dirty worker
/// at most once per `interval`. The `RunningWorkers` recovery index is always updated
/// synchronously (it is the authoritative resume index after a crash/reshard) regardless of this
/// setting. When `enabled` is `false`, the blob is written synchronously on every status change
/// (the historical behaviour), still going through the same flush code path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentStatusFlushConfig {
    /// Whether background-batched blob flushing is enabled.
    pub enabled: bool,
    /// How often the background sweeper runs (and therefore the maximum staleness of a dirty
    /// worker's cached blob). Ignored when `enabled` is `false`.
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    /// Maximum number of workers flushed concurrently in a single sweeper tick.
    pub max_concurrency: usize,
}

impl SafeDisplay for AgentStatusFlushConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "enabled: {}", self.enabled);
        let _ = writeln!(&mut result, "interval: {:?}", self.interval);
        let _ = writeln!(&mut result, "max concurrency: {}", self.max_concurrency);
        result
    }
}

impl Default for AgentStatusFlushConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(1),
            max_concurrency: 128,
        }
    }
}

/// Controls the bounded in-memory and physical invocation-result lookup structures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvocationResultsConfig {
    /// Maximum number of recent invocation results retained exactly in agent status.
    pub recent_capacity: usize,
    /// Number of bits in the persistent invocation-result Bloom filter.
    pub bloom_bits: usize,
    /// Number of hash probes used by the persistent invocation-result Bloom filter.
    pub bloom_hashes: u8,
    /// Maximum number of oplog entries processed by one physical-index catch-up step.
    pub physical_index_catch_up_chunk_size: u64,
    /// Maximum number of invocation results hydrated from the oplog and retained in memory.
    pub hydrated_cache_capacity: usize,
}

impl InvocationResultsConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.bloom_bits > 0,
            "invocation result Bloom filter must not be empty"
        );
        anyhow::ensure!(
            self.bloom_hashes > 0,
            "invocation result Bloom filter must use at least one hash"
        );
        anyhow::ensure!(
            self.physical_index_catch_up_chunk_size > 0,
            "invocation result physical-index catch-up chunk size must be at least one"
        );
        Ok(())
    }

    pub fn membership(&self) -> InvocationResultMembership {
        InvocationResultMembership::new(self.recent_capacity, self.bloom_bits, self.bloom_hashes)
    }
}

impl SafeDisplay for InvocationResultsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "recent capacity: {}", self.recent_capacity);
        let _ = writeln!(&mut result, "bloom bits: {}", self.bloom_bits);
        let _ = writeln!(&mut result, "bloom hashes: {}", self.bloom_hashes);
        let _ = writeln!(
            &mut result,
            "physical index catch-up chunk size: {}",
            self.physical_index_catch_up_chunk_size
        );
        let _ = writeln!(
            &mut result,
            "hydrated cache capacity: {}",
            self.hydrated_cache_capacity
        );
        result
    }
}

impl Default for InvocationResultsConfig {
    fn default() -> Self {
        Self {
            recent_capacity: DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY,
            bloom_bits: DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
            bloom_hashes: DEFAULT_INVOCATION_RESULT_BLOOM_HASHES,
            physical_index_catch_up_chunk_size: 1024,
            hydrated_cache_capacity: 1024,
        }
    }
}

/// Controls the *clean* cached `AgentStatusRecord` checkpoint.
///
/// The checkpoint is a separate copy of the status written only at structurally clean boundaries
/// (snapshot save, throttled idle) where no jumpable oplog region is open, and is never advanced by
/// the background flusher. It lets a status recompute fold forward from a baseline that predates any
/// later jump region instead of re-reading the whole oplog from index 1 (the previous behaviour on
/// every jump-induced detach). When `enabled` is `false`, no checkpoint is written and recompute
/// falls back to a full from-scratch fold.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentStatusCheckpointConfig {
    /// Whether clean status checkpoints are written.
    pub enabled: bool,
    /// Minimum number of oplog entries that must have been appended since the last checkpoint before
    /// a throttled (idle-boundary) checkpoint is written. Snapshot-aligned checkpoints bypass this
    /// throttle. A worker that has never been checkpointed is always eligible.
    pub min_oplog_delta: u64,
}

impl SafeDisplay for AgentStatusCheckpointConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "enabled: {}", self.enabled);
        let _ = writeln!(&mut result, "min oplog delta: {}", self.min_oplog_delta);
        result
    }
}

impl Default for AgentStatusCheckpointConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_oplog_delta: 1000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(with = "humantime_serde")]
    pub refresh_interval: Duration,
    pub claim_batch_size: u32,
    #[serde(with = "humantime_serde")]
    pub lease_ttl: Duration,
    pub max_batches_per_tick: u32,
    pub max_concurrent_action_processing: u32,
}

impl SafeDisplay for SchedulerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "refresh interval: {:?}", self.refresh_interval);
        let _ = writeln!(&mut result, "claim batch size: {}", self.claim_batch_size);
        let _ = writeln!(&mut result, "lease ttl: {:?}", self.lease_ttl);
        let _ = writeln!(
            &mut result,
            "max batches per tick: {}",
            self.max_batches_per_tick
        );
        let _ = writeln!(
            &mut result,
            "max concurrent action processing: {}",
            self.max_concurrent_action_processing
        );
        result
    }
}

/// Per-worker read-only method result cache configuration.
///
/// `max_entry_age` is the background eviction TTL, independent of the
/// per-method `CachePolicy::Ttl` semantic expiry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadOnlyCacheConfig {
    pub cache_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub max_entry_age: Duration,
    #[serde(with = "humantime_serde")]
    pub cache_eviction_interval: Duration,
}

impl Default for ReadOnlyCacheConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 256,
            max_entry_age: Duration::from_secs(2 * 60 * 60),
            cache_eviction_interval: Duration::from_secs(60),
        }
    }
}

impl SafeDisplay for ReadOnlyCacheConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "cache_capacity: {}", self.cache_capacity);
        let _ = writeln!(&mut result, "max_entry_age: {:?}", self.max_entry_age);
        let _ = writeln!(
            &mut result,
            "cache_eviction_interval: {:?}",
            self.cache_eviction_interval
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct EngineConfig {
    pub enable_fs_cache: bool,
}

impl SafeDisplay for EngineConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "enable fs cache: {}", self.enable_fs_cache);
        result
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_active_agents: 1024,
            invocation_result_broadcast_capacity: 100000,
            live_stream_event_broadcast_capacity: NonZeroUsize::new(32).unwrap(),
            max_concurrent_streams: 1024,
            event_broadcast_capacity: 1024,
            event_history_size: 128,
            fuel_to_borrow: 10000,
            ephemeral_fuel_overdraft_multiplier: 100,
            epoch_interval: Duration::from_millis(10),
            epoch_ticks: 1,
            max_oplog_query_pages_size: 100,
            max_oplog_query_page_bytes: 8 * 1024 * 1024,
            max_inline_oplog_entry_size: 1024 * 1024,
            max_concurrent_oplog_entry_reads: 4,
            max_invocation_context_stack_depth: 1024,
            max_tool_attachment_bytes: 16 * 1024 * 1024,
            max_invocation_duration: None,
            tail_work_settle_timeout: Duration::from_secs(30),
        }
    }
}

impl Default for SuspendConfig {
    fn default() -> Self {
        Self {
            suspend_after: Duration::from_secs(10),
            ephemeral_max_sleep: Duration::from_secs(60),
            wait_suspend_grace: Duration::from_secs(1),
            wait_suspend_check_interval: Duration::from_secs(10),
            rpc_suspend_after: Duration::from_secs(30),
            rpc_resume_after: Duration::from_secs(5),
        }
    }
}

impl Default for ActiveAgentsConfig {
    fn default() -> Self {
        Self {
            drop_when_full: 0.25,
            ttl: Duration::from_secs(60 * 60 * 8),
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            refresh_interval: Duration::from_secs(2),
            claim_batch_size: 100,
            lease_ttl: Duration::from_secs(30),
            max_batches_per_tick: 10,
            max_concurrent_action_processing: 64,
        }
    }
}
