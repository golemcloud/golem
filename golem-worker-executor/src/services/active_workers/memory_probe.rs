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

//! Platform-abstracted probe of the executor's real memory usage and limit.
//!
//! Reports the measured resident memory and hard limit of the process's
//! environment, used as the authoritative input to admission decisions (in
//! contrast to the estimate-based semaphore in [`super::ActiveWorkers`]).
//!
//! The trait is abstract over where the limit comes from: a containerised Linux
//! deployment reads it from the cgroup, an unconstrained process reads host RAM,
//! a configured override pins it explicitly. Backend fidelity is asymmetric —
//! cgroup v2 gives the exact kernel-enforced number; other targets fall back to
//! best-effort process RSS via [`ProcessRssProbe`] until dedicated macOS and
//! Windows backends land.

use std::fmt::Debug;

/// A snapshot of the executor environment's memory state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    /// Hard ceiling: cgroup `memory.max` on constrained Linux, configured cap
    /// or host RAM otherwise. Reaching this with `current` triggers an
    /// OOM-kill.
    pub limit_bytes: u64,
    /// Currently-resident bytes: cgroup `memory.current` on Linux (touched
    /// pages, lagging but exact), process RSS otherwise.
    pub current_bytes: u64,
}

impl MemorySnapshot {
    /// Bytes between current usage and the hard limit. Saturating: never
    /// underflows if `current` momentarily exceeds the reported `limit`.
    pub fn headroom_bytes(&self) -> u64 {
        self.limit_bytes.saturating_sub(self.current_bytes)
    }
}

/// Reads the executor environment's real memory state. Cheap enough to sample
/// at admission time, but not on every wasmtime `memory.grow` (that is what the
/// estimate-semaphore pre-check absorbs).
pub trait MemoryProbe: Send + Sync + Debug {
    fn snapshot(&self) -> MemorySnapshot;

    fn limit_bytes(&self) -> u64 {
        self.snapshot().limit_bytes
    }

    fn current_bytes(&self) -> u64 {
        self.snapshot().current_bytes
    }

    fn headroom_bytes(&self) -> u64 {
        self.snapshot().headroom_bytes()
    }
}

/// A probe whose limit is fixed at construction and whose current usage comes
/// from cross-platform process RSS via `sysinfo`.
///
/// This is the best-effort fallback used wherever no higher-fidelity backend
/// is available yet (notably macOS and Windows). It is also used when a
/// `system_memory_override` pins the limit explicitly.
#[derive(Debug)]
pub struct ProcessRssProbe {
    limit_bytes: u64,
}

impl ProcessRssProbe {
    pub fn new(limit_bytes: u64) -> Self {
        Self { limit_bytes }
    }

    fn current_rss() -> u64 {
        let mut sysinfo = sysinfo::System::new();
        let pid = sysinfo::Pid::from_u32(std::process::id());
        sysinfo.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        sysinfo.process(pid).map(|p| p.memory()).unwrap_or_default()
    }
}

impl MemoryProbe for ProcessRssProbe {
    fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            limit_bytes: self.limit_bytes,
            current_bytes: Self::current_rss(),
        }
    }
}

/// Linux cgroup v2 probe. Reads `memory.max` and `memory.current` from the
/// process's cgroup.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct CgroupV2Probe {
    /// Resolved path to the cgroup directory, e.g. `/sys/fs/cgroup`.
    base: std::path::PathBuf,
    /// Fallback limit used when `memory.max` reads `max` (unlimited) — usually
    /// host RAM or the configured override.
    fallback_limit_bytes: u64,
}

#[cfg(target_os = "linux")]
impl CgroupV2Probe {
    const DEFAULT_BASE: &'static str = "/sys/fs/cgroup";

    /// Attempts to construct a cgroup v2 probe. Returns `None` when the host is
    /// not running cgroup v2 (no unified `memory.current` at the base path), so
    /// the caller can fall back to [`ProcessRssProbe`].
    pub fn try_new(fallback_limit_bytes: u64) -> Option<Self> {
        let base = std::path::PathBuf::from(Self::DEFAULT_BASE);
        // cgroup v2 unified hierarchy exposes memory.current directly at the
        // delegated cgroup path. If it is not readable we are not on v2.
        if std::fs::read_to_string(base.join("memory.current")).is_ok() {
            Some(Self {
                base,
                fallback_limit_bytes,
            })
        } else {
            None
        }
    }

    fn read_u64(&self, file: &str) -> Option<u64> {
        let raw = std::fs::read_to_string(self.base.join(file)).ok()?;
        raw.trim().parse::<u64>().ok()
    }

    fn read_limit(&self) -> u64 {
        // memory.max contains either a number of bytes or the literal "max".
        match std::fs::read_to_string(self.base.join("memory.max")) {
            Ok(raw) => {
                let trimmed = raw.trim();
                if trimmed == "max" {
                    self.fallback_limit_bytes
                } else {
                    trimmed.parse::<u64>().unwrap_or(self.fallback_limit_bytes)
                }
            }
            Err(_) => self.fallback_limit_bytes,
        }
    }
}

#[cfg(target_os = "linux")]
impl MemoryProbe for CgroupV2Probe {
    fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            limit_bytes: self.read_limit(),
            current_bytes: self.read_u64("memory.current").unwrap_or(0),
        }
    }
}

/// Constructs the best available probe for the current platform.
///
/// On Linux, prefers cgroup v2; falls back to process RSS. On other targets,
/// uses process RSS until dedicated backends land. `limit_bytes` is the limit
/// to charge against and is also the fallback when the cgroup reports an
/// unlimited `memory.max`.
pub fn default_probe(limit_bytes: u64) -> Box<dyn MemoryProbe> {
    #[cfg(target_os = "linux")]
    {
        if let Some(probe) = CgroupV2Probe::try_new(limit_bytes) {
            return Box::new(probe);
        }
    }
    Box::new(ProcessRssProbe::new(limit_bytes))
}
