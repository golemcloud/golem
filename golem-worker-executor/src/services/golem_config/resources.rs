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
use golem_common::model::RetryConfig;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub system_memory_override: Option<u64>,
    pub worker_memory_ratio: f64,
    /// Multiplier applied to a component's `component_size` when reserving its
    /// compiled-module memory with the admission gate, charged once per resident
    /// component (shared across all its workers) rather than per worker.
    pub component_size_coefficient: f64,
    /// Whether the measured-headroom admission gate is active. Requires the
    /// executor to own its memory environment (its own cgroup/process), as in a
    /// production pod. Disable in shared environments — such as the in-process
    /// test harness — where the probe cannot isolate this executor's footprint
    /// from co-resident processes.
    pub enable_measured_admission: bool,
    #[serde(with = "humantime_serde")]
    pub acquire_retry_delay: Duration,
    pub oom_retry_config: RetryConfig,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceUsageMeteringConfig {
    /// Measures Wasmtime fuel and exports it as compute usage. When disabled, cumulative fuel is
    /// explicitly unlimited and invocation results report zero consumed fuel.
    pub compute: bool,
    /// Measures allocated linear-memory byte-time. Memory limits and admission remain enabled.
    pub memory: bool,
    /// Measures authoritative filesystem allocated-byte-time. Filesystem quotas and pressure
    /// recovery remain enabled.
    pub filesystem: bool,
}

impl ResourceUsageMeteringConfig {
    pub const fn all_enabled() -> Self {
        Self {
            compute: true,
            memory: true,
            filesystem: true,
        }
    }

    pub const fn any_byte_time_enabled(self) -> bool {
        self.memory || self.filesystem
    }
}

impl SafeDisplay for ResourceUsageMeteringConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "compute: {}", self.compute);
        let _ = writeln!(&mut result, "memory: {}", self.memory);
        let _ = writeln!(&mut result, "filesystem: {}", self.filesystem);
        result
    }
}

impl MemoryConfig {
    /// The admission policy for the measured-headroom gate. Reuses
    /// `worker_memory_ratio` as the usable fraction of the measured limit (the
    /// host keeps the remainder).
    pub(crate) fn admission_policy(
        &self,
    ) -> crate::services::active_agents::admission::AdmissionPolicy {
        crate::services::active_agents::admission::AdmissionPolicy {
            usable_ratio: self.worker_memory_ratio,
        }
    }
}

impl SafeDisplay for MemoryConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        if let Some(ovrd) = &self.system_memory_override {
            let _ = writeln!(&mut result, "system memory override: {ovrd}");
        }
        let _ = writeln!(
            &mut result,
            "worker memory ratio: {}",
            self.worker_memory_ratio
        );
        let _ = writeln!(
            &mut result,
            "component size coefficient: {}",
            self.component_size_coefficient
        );
        let _ = writeln!(
            &mut result,
            "measured admission enabled: {}",
            self.enable_measured_admission
        );
        let _ = writeln!(
            &mut result,
            "acquire retry delay: {:?}",
            self.acquire_retry_delay
        );
        let _ = writeln!(&mut result, "oom retry config:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.oom_retry_config.to_safe_string_indented()
        );

        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ResourceLimitsConfig {
    Grpc(ResourceLimitsGrpcConfig),
    Disabled(ResourceLimitsDisabledConfig),
}

impl SafeDisplay for ResourceLimitsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            ResourceLimitsConfig::Grpc(grpc) => {
                let _ = writeln!(&mut result, "grpc:");
                let _ = writeln!(&mut result, "{}", grpc.to_safe_string_indented());
            }
            ResourceLimitsConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLimitsGrpcConfig {
    #[serde(with = "humantime_serde")]
    pub batch_update_interval: Duration,
    /// How long a cached account entry may go without a server refresh before
    /// it is considered stale.
    #[serde(with = "humantime_serde")]
    pub limit_refresh_interval: Duration,
}

impl SafeDisplay for ResourceLimitsGrpcConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "batch update interval: {:?}",
            self.batch_update_interval
        );
        let _ = writeln!(
            &mut result,
            "limit refresh interval: {:?}",
            self.limit_refresh_interval
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLimitsDisabledConfig {}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            system_memory_override: None,
            worker_memory_ratio: 0.8,
            component_size_coefficient: 2.0,
            enable_measured_admission: true,
            acquire_retry_delay: Duration::from_millis(500),
            oom_retry_config: RetryConfig {
                max_attempts: u32::MAX,
                min_delay: Duration::from_millis(100),
                max_delay: Duration::from_secs(5),
                multiplier: 2.0,
                max_jitter_factor: None, // TODO: should we add jitter here?
            },
        }
    }
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self::Grpc(ResourceLimitsGrpcConfig {
            batch_update_interval: Duration::from_secs(60),
            limit_refresh_interval: Duration::from_secs(300),
        })
    }
}

/// Configuration for the executor-side quota enforcement service.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuotaServiceConfig {
    /// How often to renew quota leases with the shard manager.
    #[serde(with = "humantime_serde")]
    pub renewal_interval: Duration,
    /// When leases should be renewed before they expire.
    /// Leases will be renewed if now >= lease_expires_at - renewal_threshold
    #[serde(with = "humantime_serde")]
    pub renewal_threshold: Duration,
    /// Maximum wait time for inline throttling before returning
    /// `InsufficientAllocation` to the caller (which may then suspend).
    /// Reservations whose estimated wait exceeds this threshold are not
    /// held in the waiter queue.
    #[serde(with = "humantime_serde")]
    pub inline_wait_threshold: Duration,
}

impl SafeDisplay for QuotaServiceConfig {
    fn to_safe_string(&self) -> String {
        use std::fmt::Write;
        let mut result = String::new();
        let _ = writeln!(&mut result, "renewal interval: {:?}", self.renewal_interval);
        let _ = writeln!(
            &mut result,
            "renewal threshold: {:?}",
            self.renewal_threshold
        );
        let _ = writeln!(
            &mut result,
            "inline wait threshold: {:?}",
            self.inline_wait_threshold
        );
        result
    }
}

impl Default for QuotaServiceConfig {
    fn default() -> Self {
        Self {
            renewal_interval: Duration::from_secs(10),
            renewal_threshold: Duration::from_secs(20),
            inline_wait_threshold: Duration::from_mins(1),
        }
    }
}
