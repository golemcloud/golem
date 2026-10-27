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
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::time::Duration;
use tracing::warn;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OplogConfig {
    pub max_operations_before_commit: u64,
    pub max_operations_before_commit_ephemeral: u64,
    pub max_payload_size: usize,
    /// Maximum number of batches of entries that the primary oplog committed by itself, on its
    /// commit threshold, kept in memory until the status fold reads them. The fold reads the
    /// batches beyond it back from storage.
    pub max_retained_auto_commit_batches: usize,
    pub indexed_storage_layers: usize,
    pub blob_storage_layers: usize,
    pub entry_count_limit: u64,
    #[serde(with = "humantime_serde")]
    pub archive_interval: Duration,
    pub default_snapshotting: SnapshotPolicy,
    pub oplog_processor_snapshotting: SnapshotPolicy,
    /// Maximum number of oplog commits before the ForwardingOplog flushes
    /// buffered entries to oplog processor plugins.
    pub plugin_max_commit_count: usize,
    /// Maximum elapsed time before the ForwardingOplog flushes buffered
    /// entries to oplog processor plugins.
    #[serde(with = "humantime_serde")]
    pub plugin_max_elapsed_time: Duration,
    /// When true, wraps the oplog service with a per-account rate-limiting layer that
    /// throttles `add` calls according to each account's plan limit
    /// (`oplog_writes_per_second`). Defaults to false (disabled).
    #[serde(default)]
    pub oplog_rate_limit_enabled: bool,
    /// Controls the background sweep that archives the oplogs of agents which have gone quiet.
    pub sweep: OplogSweepConfig,
}

impl SafeDisplay for OplogConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "max operations before commit: {}",
            self.max_operations_before_commit
        );
        let _ = writeln!(
            &mut result,
            "max operations before commit for ephemerals: {}",
            self.max_operations_before_commit_ephemeral
        );
        let _ = writeln!(&mut result, "max payload size: {}", self.max_payload_size);
        let _ = writeln!(
            &mut result,
            "max retained auto-commit batches: {}",
            self.max_retained_auto_commit_batches
        );
        let _ = writeln!(
            &mut result,
            "indexed storage layers: {}",
            self.indexed_storage_layers
        );
        let _ = writeln!(
            &mut result,
            "blob storage layers: {}",
            self.blob_storage_layers
        );
        let _ = writeln!(&mut result, "entry count limit: {}", self.entry_count_limit);
        let _ = writeln!(&mut result, "archive interval: {:?}", self.archive_interval);
        let _ = writeln!(&mut result, "default snapshotting:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.default_snapshotting.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "oplog processor snapshotting:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.oplog_processor_snapshotting.to_safe_string_indented()
        );
        let _ = writeln!(
            &mut result,
            "plugin max commit count: {}",
            self.plugin_max_commit_count
        );
        let _ = writeln!(
            &mut result,
            "plugin max elapsed time: {:?}",
            self.plugin_max_elapsed_time
        );
        let _ = writeln!(
            &mut result,
            "oplog rate limit enabled: {}",
            self.oplog_rate_limit_enabled
        );
        let _ = writeln!(&mut result, "sweep:");
        let _ = writeln!(&mut result, "{}", self.sweep.to_safe_string());
        result
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "config")]
#[derive(Default)]
pub enum SnapshotPolicy {
    #[default]
    Disabled,
    Periodic {
        #[serde(with = "humantime_serde")]
        period: Duration,
    },
    EveryNInvocation {
        count: u16,
    },
}

impl SnapshotPolicy {
    /// Normalizes the policy by disabling zero-valued configurations.
    pub fn normalize(self) -> Self {
        match &self {
            SnapshotPolicy::Disabled => self,
            SnapshotPolicy::Periodic { period } => {
                if period.is_zero() {
                    warn!("Snapshot periodic duration is zero, disabling");
                    SnapshotPolicy::Disabled
                } else {
                    self
                }
            }
            SnapshotPolicy::EveryNInvocation { count } => {
                if *count == 0 {
                    warn!("Snapshot every-n-invocation count is zero, disabling");
                    SnapshotPolicy::Disabled
                } else {
                    self
                }
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for SnapshotPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "type", content = "config")]
        enum Raw {
            Disabled,
            Periodic {
                #[serde(with = "humantime_serde")]
                period: Duration,
            },
            EveryNInvocation {
                count: u16,
            },
        }

        let raw = Raw::deserialize(deserializer)?;
        let policy = match raw {
            Raw::Disabled => SnapshotPolicy::Disabled,
            Raw::Periodic { period } => SnapshotPolicy::Periodic { period },
            Raw::EveryNInvocation { count } => SnapshotPolicy::EveryNInvocation { count },
        };
        Ok(policy.normalize())
    }
}

impl SafeDisplay for SnapshotPolicy {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            SnapshotPolicy::Disabled => {
                let _ = writeln!(&mut result, "disabled");
            }
            SnapshotPolicy::Periodic { period } => {
                let _ = writeln!(&mut result, "periodic:");
                let _ = writeln!(&mut result, "  period: {period:?}");
            }
            SnapshotPolicy::EveryNInvocation { count } => {
                let _ = writeln!(&mut result, "every n invocation:");
                let _ = writeln!(&mut result, "  count: {count}");
            }
        }
        result
    }
}

impl Default for OplogConfig {
    fn default() -> Self {
        Self {
            max_operations_before_commit: 128,
            max_operations_before_commit_ephemeral: 1024,
            max_payload_size: 64 * 1024,
            max_retained_auto_commit_batches: 32,
            indexed_storage_layers: 2,
            blob_storage_layers: 1,
            entry_count_limit: 1024,
            archive_interval: Duration::from_secs(60 * 60 * 24), // 24 hours
            default_snapshotting: SnapshotPolicy::default(),
            oplog_processor_snapshotting: SnapshotPolicy::EveryNInvocation { count: 10 },
            plugin_max_commit_count: 3,
            plugin_max_elapsed_time: Duration::from_secs(5),
            oplog_rate_limit_enabled: false,
            sweep: OplogSweepConfig::default(),
        }
    }
}

/// Controls the background sweep that archives the oplogs of ephemeral agents which have gone
/// quiet. It finds them by paginating the oplog layers, so an ephemeral invocation registers no
/// `ScheduledAction::ArchiveOplog`. Durable agents are not swept; see
/// [`oplog_sweep`](crate::services::oplog_sweep).
///
/// A tick that hits a per-tick bound keeps its scan cursor and resumes there on the next tick, so
/// work is deferred, never dropped.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OplogSweepConfig {
    /// Whether the sweep runs. Also read by `StatusState::schedule_oplog_archive_if_needed`: while
    /// it is false, ephemeral agents register `ScheduledAction::ArchiveOplog` instead, so an oplog
    /// stranded by a crashed pod always has something to move it.
    pub enabled: bool,
    /// Wait between ticks. An agent is archived once its last oplog index is unchanged across two
    /// scan passes, and a pass takes one interval only while the namespace fits in one tick's
    /// budget.
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    /// Keys read per scan call.
    pub page_size: u64,
    /// Agents archived concurrently. Shares the indexed-storage connection budget with
    /// invocations, and bounds memory: an archive step reads an agent's whole layer into one `Vec`.
    pub max_concurrency: usize,
    /// Agents one tick may archive, split across routes. A page is decided as a unit, so a tick can
    /// exceed this by up to `page_size`.
    pub max_archives_per_tick: usize,
    /// Keys one tick may scan, split across routes. A close bound rather than an exact one, since
    /// Redis and the multi-file SQLite backend can return more keys than asked for.
    pub max_scanned_per_tick: usize,
    /// Wall-clock bound on one tick. The count budgets bound work; this bounds how long a tick
    /// holds the indexed-storage concurrency it shares with invocations, which matters when the
    /// store is slow. A tick stops at its next boundary, never inside an agent's archive, so it
    /// can overrun by the agents already started, at most `max_concurrency` of them.
    #[serde(with = "humantime_serde")]
    pub max_tick_duration: Duration,
    /// Most intervals to wait after a tick that hit `max_tick_duration`. The wait doubles after
    /// each such tick and resets once a tick finishes in time, so the sweep backs off a slow store.
    pub max_backoff_intervals: u32,
    /// Most agents whose previous index is remembered. An agent past the bound goes untracked for
    /// that pass and is archived a pass later, so set this above the largest backlog of stranded
    /// oplogs one pod should work through.
    pub max_tracked_agents: usize,
}

impl SafeDisplay for OplogSweepConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "enabled: {}", self.enabled);
        let _ = writeln!(&mut result, "interval: {:?}", self.interval);
        let _ = writeln!(&mut result, "page size: {}", self.page_size);
        let _ = writeln!(&mut result, "max concurrency: {}", self.max_concurrency);
        let _ = writeln!(
            &mut result,
            "max archives per tick: {}",
            self.max_archives_per_tick
        );
        let _ = writeln!(
            &mut result,
            "max scanned per tick: {}",
            self.max_scanned_per_tick
        );
        let _ = writeln!(
            &mut result,
            "max tick duration: {:?}",
            self.max_tick_duration
        );
        let _ = writeln!(
            &mut result,
            "max backoff intervals: {}",
            self.max_backoff_intervals
        );
        let _ = writeln!(
            &mut result,
            "max tracked agents: {}",
            self.max_tracked_agents
        );
        result
    }
}

impl Default for OplogSweepConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(60),
            page_size: 128,
            max_concurrency: 4,
            max_archives_per_tick: 256,
            max_scanned_per_tick: 4096,
            max_tick_duration: Duration::from_secs(30),
            max_backoff_intervals: 8,
            max_tracked_agents: 100_000,
        }
    }
}
