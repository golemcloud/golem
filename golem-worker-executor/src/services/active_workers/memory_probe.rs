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
//! Production probes serve the last coherent snapshot while refreshing on a
//! blocking worker, so guest memory growth never performs operating-system I/O
//! on a Tokio reactor or fails solely because a probe read is slow.
//!
//! The trait is abstract over where the limit comes from: a containerised Linux
//! deployment reads it from the cgroup, an unconstrained process reads host RAM,
//! a configured override pins it explicitly. Backend fidelity is asymmetric —
//! cgroup v2 gives the exact kernel-enforced number; other targets fall back to
//! best-effort process RSS via [`ProcessRssProbe`] until dedicated macOS and
//! Windows backends land.

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const SNAPSHOT_CACHE_TTL: Duration = Duration::from_millis(10);

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

    /// Bytes between current usage and the hard limit. Saturating: never
    /// underflows if `current` momentarily exceeds the reported `limit`.
    pub fn headroom_bytes(&self) -> u64 {
        self.limit_bytes.saturating_sub(self.current_bytes)
    }
}

/// Reads the executor environment's real memory state.
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

#[derive(Debug)]
struct CachedMemoryProbe {
    inner: Arc<CachedMemoryProbeInner>,
}

#[derive(Debug)]
struct CachedMemoryProbeInner {
    source: Arc<dyn MemoryProbe>,
    started_at: Instant,
    refresh_interval_nanos: u64,
    last_refresh_nanos: AtomicU64,
    snapshot: arc_swap::ArcSwap<MemorySnapshot>,
    refresh_in_progress: AtomicBool,
}

struct RefreshGuard {
    inner: Arc<CachedMemoryProbeInner>,
    completed: bool,
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.inner.last_refresh_nanos.store(
                CachedMemoryProbe::elapsed_nanos(&self.inner),
                Ordering::Release,
            );
        }
        self.inner
            .refresh_in_progress
            .store(false, Ordering::Release);
    }
}

impl CachedMemoryProbe {
    fn new(source: Box<dyn MemoryProbe>, refresh_interval: Duration) -> Self {
        let source = Arc::<dyn MemoryProbe>::from(source);
        let snapshot = source.snapshot();
        Self {
            inner: Arc::new(CachedMemoryProbeInner {
                source,
                started_at: Instant::now(),
                refresh_interval_nanos: refresh_interval.as_nanos().min(u64::MAX as u128) as u64,
                last_refresh_nanos: AtomicU64::new(0),
                snapshot: arc_swap::ArcSwap::new(Arc::new(snapshot)),
                refresh_in_progress: AtomicBool::new(false),
            }),
        }
    }

    fn elapsed_nanos(inner: &CachedMemoryProbeInner) -> u64 {
        inner.started_at.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }

    fn refresh(mut guard: RefreshGuard) {
        let inner = &guard.inner;
        let snapshot = inner.source.snapshot();
        inner.snapshot.store(Arc::new(snapshot));
        inner
            .last_refresh_nanos
            .store(Self::elapsed_nanos(&inner), Ordering::Release);
        guard.completed = true;
    }

    fn refresh_if_stale(&self) {
        let now = Self::elapsed_nanos(&self.inner);
        let last_refresh = self.inner.last_refresh_nanos.load(Ordering::Acquire);
        if now.saturating_sub(last_refresh) < self.inner.refresh_interval_nanos
            || self
                .inner
                .refresh_in_progress
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }

        let guard = RefreshGuard {
            inner: self.inner.clone(),
            completed: false,
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(move || Self::refresh(guard));
        } else {
            Self::refresh(guard);
        }
    }
}

impl MemoryProbe for CachedMemoryProbe {
    fn snapshot(&self) -> MemorySnapshot {
        self.refresh_if_stale();
        **self.inner.snapshot.load()
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
            Ok(raw) => raw
                .trim()
                .parse::<u64>()
                .unwrap_or(self.fallback_limit_bytes),
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
        return Box::new(CachedMemoryProbe::new(
            Box::new(ProcessRssProbe::new(limit)),
            SNAPSHOT_CACHE_TTL,
        ));
    }

    let host_ram = {
        let mut sysinfo = sysinfo::System::new();
        sysinfo.refresh_memory();
        sysinfo.total_memory()
    };

    #[cfg(target_os = "linux")]
    {
        if let Some(probe) = CgroupV2Probe::try_new(host_ram) {
            let probe = CachedMemoryProbe::new(Box::new(probe), SNAPSHOT_CACHE_TTL);
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
    Box::new(CachedMemoryProbe::new(
        Box::new(ProcessRssProbe::new(host_ram)),
        SNAPSHOT_CACHE_TTL,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex};
    use test_r::test;

    #[derive(Debug)]
    struct CountingProbe {
        reads: Arc<AtomicU64>,
        limit_bytes: Arc<AtomicU64>,
        current_bytes: Arc<AtomicU64>,
        refresh_gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl MemoryProbe for CountingProbe {
        fn snapshot(&self) -> MemorySnapshot {
            if self.reads.fetch_add(1, Ordering::Relaxed) > 0 {
                let (released, ready) = &*self.refresh_gate;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = ready.wait(released).unwrap();
                }
            }
            MemorySnapshot {
                limit_bytes: self.limit_bytes.load(Ordering::Relaxed),
                current_bytes: self.current_bytes.load(Ordering::Relaxed),
            }
        }
    }

    #[derive(Debug)]
    struct PanickingProbe {
        reads: Arc<AtomicU64>,
    }

    impl MemoryProbe for PanickingProbe {
        fn snapshot(&self) -> MemorySnapshot {
            let read = self.reads.fetch_add(1, Ordering::Relaxed);
            if read == 1 {
                panic!("simulated probe failure");
            }
            MemorySnapshot {
                limit_bytes: 100,
                current_bytes: if read == 0 { 1 } else { 42 },
            }
        }
    }

    fn stale_cached_probe(
        source: Arc<dyn MemoryProbe>,
        snapshot: MemorySnapshot,
    ) -> CachedMemoryProbe {
        let refresh_interval = Duration::from_secs(60);
        CachedMemoryProbe {
            inner: Arc::new(CachedMemoryProbeInner {
                source,
                started_at: Instant::now() - refresh_interval - Duration::from_secs(1),
                refresh_interval_nanos: refresh_interval.as_nanos() as u64,
                last_refresh_nanos: AtomicU64::new(0),
                snapshot: arc_swap::ArcSwap::new(Arc::new(snapshot)),
                refresh_in_progress: AtomicBool::new(false),
            }),
        }
    }

    #[test]
    async fn cached_probe_coalesces_reads_and_refreshes_off_thread() {
        let reads = Arc::new(AtomicU64::new(1));
        let limit_bytes = Arc::new(AtomicU64::new(100));
        let current_bytes = Arc::new(AtomicU64::new(1));
        let refresh_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let probe = stale_cached_probe(
            Arc::new(CountingProbe {
                reads: reads.clone(),
                limit_bytes: limit_bytes.clone(),
                current_bytes: current_bytes.clone(),
                refresh_gate: refresh_gate.clone(),
            }),
            MemorySnapshot {
                limit_bytes: 100,
                current_bytes: 1,
            },
        );

        limit_bytes.store(50, Ordering::Relaxed);
        current_bytes.store(42, Ordering::Relaxed);
        assert_eq!(probe.snapshot().current_bytes, 1);

        tokio::time::timeout(Duration::from_secs(1), async {
            while reads.load(Ordering::Acquire) != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for _ in 0..100 {
            assert_eq!(probe.snapshot().current_bytes, 1);
        }
        assert_eq!(
            reads.load(Ordering::Acquire),
            2,
            "concurrent reads must share the refresh in progress"
        );
        *refresh_gate.0.lock().unwrap() = true;
        refresh_gate.1.notify_one();

        tokio::time::timeout(Duration::from_secs(1), async {
            while probe.snapshot().limit_bytes != 50 || probe.snapshot().current_bytes != 42 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            probe.snapshot(),
            MemorySnapshot {
                limit_bytes: 50,
                current_bytes: 42,
            }
        );
        assert_eq!(reads.load(Ordering::Relaxed), 2);
    }

    #[test]
    async fn cached_probe_retries_after_refresh_panics() {
        let reads = Arc::new(AtomicU64::new(1));
        let probe = stale_cached_probe(
            Arc::new(PanickingProbe {
                reads: reads.clone(),
            }),
            MemorySnapshot {
                limit_bytes: 100,
                current_bytes: 1,
            },
        );

        assert_eq!(probe.snapshot().current_bytes, 1);

        tokio::time::timeout(Duration::from_secs(1), async {
            while probe.inner.refresh_in_progress.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
            probe.inner.last_refresh_nanos.store(0, Ordering::Release);
            while probe.snapshot().current_bytes != 42 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(reads.load(Ordering::Relaxed), 3);
    }

    #[test]
    async fn cached_probe_backs_off_after_refresh_panics() {
        let reads = Arc::new(AtomicU64::new(1));
        let probe = stale_cached_probe(
            Arc::new(PanickingProbe {
                reads: reads.clone(),
            }),
            MemorySnapshot {
                limit_bytes: 100,
                current_bytes: 1,
            },
        );

        assert_eq!(probe.snapshot().current_bytes, 1);
        tokio::time::timeout(Duration::from_secs(1), async {
            while probe.inner.refresh_in_progress.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        for _ in 0..100 {
            assert_eq!(probe.snapshot().current_bytes, 1);
        }
        assert_eq!(
            reads.load(Ordering::Relaxed),
            2,
            "a failed refresh must not be retried before the refresh interval elapses"
        );
    }
}
