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
//! environment, used as the authoritative input to admission decisions.
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
    pub fn usable_limit_bytes(&self, usable_ratio: f64) -> u64 {
        (self.limit_bytes as f64 * usable_ratio) as u64
    }

    pub fn usable_headroom_bytes(&self, usable_ratio: f64) -> u64 {
        self.usable_limit_bytes(usable_ratio)
            .saturating_sub(self.current_bytes)
    }

    /// Bytes between current usage and the hard limit. Saturating: never
    /// underflows if `current` momentarily exceeds the reported `limit`.
    pub fn headroom_bytes(&self) -> u64 {
        self.limit_bytes.saturating_sub(self.current_bytes)
    }
}

/// Reads the executor environment's real memory state.
pub trait MemoryProbe: Send + Sync + Debug {
    fn snapshot(&self) -> MemorySnapshot;

    fn snapshot_for_ratio(&self, usable_ratio: f64) -> MemorySnapshot;

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

    fn snapshot_for_ratio(&self, _usable_ratio: f64) -> MemorySnapshot {
        self.snapshot()
    }
}

/// A probe with a fixed limit and a fixed current usage, both set at
/// construction. Reports the same snapshot on every call regardless of the
/// host. Used by the in-process test harness, where the executor shares its
/// process (and therefore its real RSS) with the test framework and other
/// services, so a process-RSS probe cannot isolate this executor's footprint.
/// Pinning `current_bytes` to a known value (typically 0) makes the gate decide
/// purely on the granted accounting against the pinned limit, which is exact and
/// process-isolated, so memory-pressure tests are deterministic.
#[derive(Debug)]
pub struct FixedProbe {
    limit_bytes: u64,
    current_bytes: u64,
}

impl FixedProbe {
    pub fn new(limit_bytes: u64, current_bytes: u64) -> Self {
        Self {
            limit_bytes,
            current_bytes,
        }
    }
}

impl MemoryProbe for FixedProbe {
    fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            limit_bytes: self.limit_bytes,
            current_bytes: self.current_bytes,
        }
    }

    fn snapshot_for_ratio(&self, _usable_ratio: f64) -> MemorySnapshot {
        self.snapshot()
    }
}

/// Linux cgroup v2 probe. Reads `memory.max` and `memory.current` from the
/// process's cgroup.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct CgroupV2Probe {
    /// Resolved path to the cgroup directory, e.g. `/sys/fs/cgroup`.
    base: std::path::PathBuf,
    hierarchy: std::path::PathBuf,
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
        let hierarchy = std::path::PathBuf::from(Self::DEFAULT_BASE);
        let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
        let base = resolve_cgroup_v2_path(&hierarchy, &cgroup)?;
        // cgroup v2 unified hierarchy exposes memory.current directly at the
        // delegated cgroup path. If it is not readable we are not on v2.
        if std::fs::read_to_string(base.join("memory.current")).is_ok() {
            Some(Self {
                base,
                hierarchy,
                fallback_limit_bytes,
            })
        } else {
            None
        }
    }

    fn read_snapshot(&self, usable_ratio: f64) -> MemorySnapshot {
        let mut path = self.base.as_path();
        let mut constrained: Option<MemorySnapshot> = None;
        loop {
            if let Ok(current) = std::fs::read_to_string(path.join("memory.current"))
                && let Ok(current_bytes) = current.trim().parse::<u64>()
            {
                let limit_bytes = std::fs::read_to_string(path.join("memory.max"))
                    .ok()
                    .and_then(|raw| raw.trim().parse::<u64>().ok())
                    .unwrap_or(self.fallback_limit_bytes);
                let candidate = MemorySnapshot {
                    limit_bytes,
                    current_bytes,
                };
                let candidate_headroom = candidate.usable_headroom_bytes(usable_ratio);
                if constrained.is_none_or(|snapshot| {
                    candidate_headroom < snapshot.usable_headroom_bytes(usable_ratio)
                }) {
                    constrained = Some(candidate);
                }
            }

            if path == self.hierarchy {
                break;
            }
            let Some(parent) = path.parent() else {
                break;
            };
            path = parent;
        }

        constrained.unwrap_or(MemorySnapshot {
            limit_bytes: self.fallback_limit_bytes,
            current_bytes: 0,
        })
    }
}

#[cfg(target_os = "linux")]
fn resolve_cgroup_v2_path(
    hierarchy: &std::path::Path,
    proc_self_cgroup: &str,
) -> Option<std::path::PathBuf> {
    let relative = proc_self_cgroup.lines().find_map(|line| {
        let (hierarchy_id, path) = line.split_once("::")?;
        (hierarchy_id == "0").then_some(path.trim_start_matches('/'))
    })?;
    Some(hierarchy.join(relative))
}

#[cfg(all(test, target_os = "linux"))]
mod cgroup_tests {
    use super::{CgroupV2Probe, MemoryProbe, MemorySnapshot, resolve_cgroup_v2_path};
    use std::path::Path;
    use tempfile::tempdir;
    use test_r::test;

    #[test]
    fn nested_cgroup_path_resolves_below_the_unified_hierarchy() {
        let cgroup = "2:memory:/legacy\n0::/system.slice/golem.service\n";

        assert_eq!(
            resolve_cgroup_v2_path(Path::new(CgroupV2Probe::DEFAULT_BASE), cgroup),
            Some(Path::new("/sys/fs/cgroup/system.slice/golem.service").to_path_buf())
        );
        assert_eq!(
            resolve_cgroup_v2_path(Path::new(CgroupV2Probe::DEFAULT_BASE), "2:memory:/legacy\n"),
            None
        );
    }

    #[test]
    fn snapshot_uses_the_constraining_ancestor() {
        let hierarchy = tempdir().unwrap();
        let parent = hierarchy.path().join("parent");
        let leaf = parent.join("leaf");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(hierarchy.path().join("memory.current"), "100\n").unwrap();
        std::fs::write(hierarchy.path().join("memory.max"), "max\n").unwrap();
        std::fs::write(parent.join("memory.current"), "450\n").unwrap();
        std::fs::write(parent.join("memory.max"), "500\n").unwrap();
        std::fs::write(leaf.join("memory.current"), "100\n").unwrap();
        std::fs::write(leaf.join("memory.max"), "1000\n").unwrap();

        let probe = CgroupV2Probe {
            base: leaf,
            hierarchy: hierarchy.path().to_path_buf(),
            fallback_limit_bytes: 2000,
        };

        assert_eq!(
            probe.snapshot(),
            MemorySnapshot {
                limit_bytes: 500,
                current_bytes: 450
            }
        );
    }

    #[test]
    fn snapshot_applies_ratio_before_selecting_the_constraining_ancestor() {
        let hierarchy = tempdir().unwrap();
        let parent = hierarchy.path().join("parent");
        let leaf = parent.join("leaf");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(hierarchy.path().join("memory.current"), "0\n").unwrap();
        std::fs::write(hierarchy.path().join("memory.max"), "max\n").unwrap();
        std::fs::write(parent.join("memory.current"), "100\n").unwrap();
        std::fs::write(parent.join("memory.max"), "500\n").unwrap();
        std::fs::write(leaf.join("memory.current"), "500\n").unwrap();
        std::fs::write(leaf.join("memory.max"), "1000\n").unwrap();

        let probe = CgroupV2Probe {
            base: leaf,
            hierarchy: hierarchy.path().to_path_buf(),
            fallback_limit_bytes: 2000,
        };

        assert_eq!(
            probe.snapshot(),
            MemorySnapshot {
                limit_bytes: 500,
                current_bytes: 100,
            }
        );
        assert_eq!(
            probe.snapshot_for_ratio(0.5),
            MemorySnapshot {
                limit_bytes: 1000,
                current_bytes: 500,
            }
        );
    }
}

#[cfg(target_os = "linux")]
impl MemoryProbe for CgroupV2Probe {
    fn snapshot(&self) -> MemorySnapshot {
        self.read_snapshot(1.0)
    }

    fn snapshot_for_ratio(&self, usable_ratio: f64) -> MemorySnapshot {
        self.read_snapshot(usable_ratio)
    }
}

/// Constructs the best available probe.
///
/// When `memory_override` is set, the limit is self-declared and treated as an
/// isolated budget measured against this process's RSS — the executor does not
/// assume it owns a cgroup. When it is `None`, the executor is assumed to own
/// its memory environment, so on Linux the exact cgroup v2 numbers are used
/// (falling back to host RAM / process RSS otherwise).
pub fn default_probe(memory_override: Option<u64>) -> Box<dyn MemoryProbe> {
    if let Some(limit) = memory_override {
        tracing::info!(
            limit_bytes = limit,
            "Memory probe: ProcessRssProbe (limit pinned by system_memory_override)"
        );
        return Box::new(ProcessRssProbe::new(limit));
    }

    let host_ram = {
        let mut sysinfo = sysinfo::System::new();
        sysinfo.refresh_memory();
        sysinfo.total_memory()
    };

    #[cfg(target_os = "linux")]
    {
        if let Some(probe) = CgroupV2Probe::try_new(host_ram) {
            let snapshot = probe.snapshot();
            tracing::info!(
                limit_bytes = snapshot.limit_bytes,
                current_bytes = snapshot.current_bytes,
                "Memory probe: CgroupV2Probe (cgroup memory.max/current)"
            );
            return Box::new(probe);
        }
    }
    tracing::info!(
        limit_bytes = host_ram,
        "Memory probe: ProcessRssProbe (host RAM, no cgroup v2 limit)"
    );
    Box::new(ProcessRssProbe::new(host_ram))
}
