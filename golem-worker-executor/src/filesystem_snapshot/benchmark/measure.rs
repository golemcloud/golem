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

//! Measures one step of a phase: wall time, CPU time, memory, threads and blob storage requests.
//!
//! The values come from the process and from the cgroup v2 files of the container. A value that
//! the host does not give is `null` in the result.

use super::report::{
    CgroupCpuTime, CpuTime, MemoryPeaks, StepRecord, StepStatus, ThreadCounts, millis,
};
use super::requests::{MeasuredBlobStorage, summarize, written_and_read};
use serde_json::Value;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The time between two samples of the memory and the threads.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(10);

const PROC_STATUS: &str = "/proc/self/status";
const PROC_CLEAR_REFS: &str = "/proc/self/clear_refs";
const CGROUP_CPU_STAT: &str = "/sys/fs/cgroup/cpu.stat";
const CGROUP_MEMORY_CURRENT: &str = "/sys/fs/cgroup/memory.current";
const CGROUP_MEMORY_STAT: &str = "/sys/fs/cgroup/memory.stat";

/// Runs the step and measures it.
///
/// The requests of the storage before the step are removed first, so the record holds only the
/// requests of the step.
pub(super) async fn measure<T>(
    name: &'static str,
    storage: &MeasuredBlobStorage,
    step: impl Future<Output = anyhow::Result<T>>,
) -> (StepRecord, anyhow::Result<T>) {
    let _ = storage.take();
    let before = ProcessSample::read();
    let peak_reset = reset_peak_rss();
    let sampler = Sampler::start();
    let started = Instant::now();
    let result = step.await;
    let wall = started.elapsed();
    let peaks = sampler.stop();
    let after = ProcessSample::read();
    let records = storage.take();
    let (bytes_written, bytes_read) = written_and_read(&records);
    let (rss_peak, rss_peak_source) = match after.status.peak_rss {
        Some(peak) if peak_reset => (peak, "VmHWM"),
        _ => (peaks.rss, "sampled VmRSS"),
    };
    let record = StepRecord {
        name,
        status: match &result {
            Ok(_) => StepStatus::Ok,
            Err(error) => StepStatus::Error(format!("{error:#}").into()),
        },
        parameters: Value::Object(Default::default()),
        wall_ms: Some(millis(wall)),
        cpu: Some(CpuTime {
            user_ms: millis(after.user.saturating_sub(before.user)),
            system_ms: millis(after.system.saturating_sub(before.system)),
            cgroup: before
                .cgroup_cpu
                .zip(after.cgroup_cpu)
                .map(|(before, after)| after.since(&before)),
        }),
        memory: Some(MemoryPeaks {
            rss_start_bytes: before.status.rss.unwrap_or_default(),
            rss_peak_bytes: rss_peak,
            rss_peak_source,
            cgroup_current_peak_bytes: peaks.cgroup_current,
            cgroup_anon_peak_bytes: peaks.cgroup_anon,
            cgroup_file_peak_bytes: peaks.cgroup_file,
        }),
        threads: Some(ThreadCounts {
            start: before.status.threads.unwrap_or_default(),
            peak: peaks.threads,
            sample_interval_ms: SAMPLE_INTERVAL.as_millis() as u64,
        }),
        requests: summarize(&records),
        bytes_written,
        bytes_read,
        phases: Box::default(),
        details: Value::Object(Default::default()),
    };
    (record, result)
}

/// The values of `/proc/self/status` that a step uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct StatusValues {
    pub(super) rss: Option<u64>,
    pub(super) peak_rss: Option<u64>,
    pub(super) threads: Option<u64>,
}

/// Reads `VmRSS`, `VmHWM` and `Threads` from the text of `/proc/<pid>/status`. The sizes are in
/// bytes.
pub(super) fn parse_status(text: &str) -> StatusValues {
    text.lines().filter_map(|line| line.split_once(':')).fold(
        StatusValues::default(),
        |values, (key, value)| {
            let number = || value.split_whitespace().next()?.parse::<u64>().ok();
            match key {
                "VmRSS" => StatusValues {
                    rss: number().map(|kib| kib * 1024),
                    ..values
                },
                "VmHWM" => StatusValues {
                    peak_rss: number().map(|kib| kib * 1024),
                    ..values
                },
                "Threads" => StatusValues {
                    threads: number(),
                    ..values
                },
                _ => values,
            }
        },
    )
}

/// The CPU counters of a cgroup v2, in microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CgroupCpu {
    pub(super) usage_usec: u64,
    pub(super) user_usec: u64,
    pub(super) system_usec: u64,
    pub(super) nr_periods: u64,
    pub(super) nr_throttled: u64,
    pub(super) throttled_usec: u64,
}

impl CgroupCpu {
    fn since(&self, before: &Self) -> CgroupCpuTime {
        let micros =
            |after: u64, before: u64| millis(Duration::from_micros(after.saturating_sub(before)));
        CgroupCpuTime {
            usage_ms: micros(self.usage_usec, before.usage_usec),
            user_ms: micros(self.user_usec, before.user_usec),
            system_ms: micros(self.system_usec, before.system_usec),
            nr_periods: self.nr_periods.saturating_sub(before.nr_periods),
            nr_throttled: self.nr_throttled.saturating_sub(before.nr_throttled),
            throttled_ms: micros(self.throttled_usec, before.throttled_usec),
        }
    }
}

/// Reads the text of a cgroup v2 `cpu.stat`. A counter that the text does not have is zero; the
/// throttling counters are only there when the cgroup has a CPU limit controller.
pub(super) fn parse_cpu_stat(text: &str) -> CgroupCpu {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .filter_map(|(key, value)| value.trim().parse::<u64>().ok().map(|value| (key, value)))
        .fold(CgroupCpu::default(), |counters, (key, value)| match key {
            "usage_usec" => CgroupCpu {
                usage_usec: value,
                ..counters
            },
            "user_usec" => CgroupCpu {
                user_usec: value,
                ..counters
            },
            "system_usec" => CgroupCpu {
                system_usec: value,
                ..counters
            },
            "nr_periods" => CgroupCpu {
                nr_periods: value,
                ..counters
            },
            "nr_throttled" => CgroupCpu {
                nr_throttled: value,
                ..counters
            },
            "throttled_usec" => CgroupCpu {
                throttled_usec: value,
                ..counters
            },
            _ => counters,
        })
}

/// Reads the value of a key of a cgroup v2 `memory.stat`, in bytes.
pub(super) fn memory_stat_value(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .find(|(name, _)| *name == key)
        .and_then(|(_, value)| value.trim().parse().ok())
}

/// The CPU time and the status of the process at one moment.
struct ProcessSample {
    user: Duration,
    system: Duration,
    status: StatusValues,
    cgroup_cpu: Option<CgroupCpu>,
}

impl ProcessSample {
    fn read() -> Self {
        let (user, system) = process_cpu_time();
        Self {
            user,
            system,
            status: read_status(),
            cgroup_cpu: read_text(CGROUP_CPU_STAT).map(|text| parse_cpu_stat(&text)),
        }
    }
}

/// Gives the user and the system CPU time of all threads of the process.
fn process_cpu_time() -> (Duration, Duration) {
    // SAFETY: `getrusage` writes one `rusage` record, which the zeroed value holds.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
            Some(usage)
        } else {
            None
        }
    };
    usage.map_or((Duration::ZERO, Duration::ZERO), |usage| {
        (time_of(usage.ru_utime), time_of(usage.ru_stime))
    })
}

fn time_of(time: libc::timeval) -> Duration {
    Duration::from_secs(u64::try_from(time.tv_sec).unwrap_or_default())
        + Duration::from_micros(u64::try_from(time.tv_usec).unwrap_or_default())
}

fn read_status() -> StatusValues {
    read_text(PROC_STATUS)
        .map(|text| parse_status(&text))
        .unwrap_or_default()
}

fn read_text(path: &str) -> Option<String> {
    std::fs::read_to_string(Path::new(path)).ok()
}

/// Sets `VmHWM` of the process to its current RSS, and tells whether that worked.
fn reset_peak_rss() -> bool {
    std::fs::write(PROC_CLEAR_REFS, b"5").is_ok()
}

/// The largest values that the sampler saw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Peaks {
    rss: u64,
    threads: u64,
    cgroup_current: Option<u64>,
    cgroup_anon: Option<u64>,
    cgroup_file: Option<u64>,
}

impl Peaks {
    fn with(self, sample: Peaks) -> Self {
        let max = |left: Option<u64>, right: Option<u64>| left.max(right);
        Self {
            rss: self.rss.max(sample.rss),
            threads: self.threads.max(sample.threads),
            cgroup_current: max(self.cgroup_current, sample.cgroup_current),
            cgroup_anon: max(self.cgroup_anon, sample.cgroup_anon),
            cgroup_file: max(self.cgroup_file, sample.cgroup_file),
        }
    }

    fn read() -> Self {
        let status = read_status();
        let memory_stat = read_text(CGROUP_MEMORY_STAT);
        Self {
            rss: status.rss.unwrap_or_default(),
            threads: status.threads.unwrap_or_default(),
            cgroup_current: read_text(CGROUP_MEMORY_CURRENT)
                .and_then(|text| text.trim().parse().ok()),
            cgroup_anon: memory_stat
                .as_deref()
                .and_then(|text| memory_stat_value(text, "anon")),
            cgroup_file: memory_stat
                .as_deref()
                .and_then(|text| memory_stat_value(text, "file")),
        }
    }
}

/// A thread that samples the memory and the threads of the process until it is stopped.
struct Sampler {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Peaks>,
}

impl Sampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let stop = stop.clone();
            move || {
                std::iter::from_fn(|| {
                    (!stop.load(Ordering::Acquire)).then(|| {
                        let sample = Peaks::read();
                        std::thread::sleep(SAMPLE_INTERVAL);
                        sample
                    })
                })
                .fold(Peaks::default(), Peaks::with)
            }
        });
        Self { stop, thread }
    }

    fn stop(self) -> Peaks {
        self.stop.store(true, Ordering::Release);
        self.thread.join().unwrap_or_default().with(Peaks::read())
    }
}

#[cfg(test)]
mod tests {
    use super::{CgroupCpu, StatusValues, memory_stat_value, parse_cpu_stat, parse_status};
    use pretty_assertions::assert_eq;
    use test_r::test;

    #[test]
    fn the_status_gives_the_rss_the_peak_rss_and_the_threads() {
        let text = "Name:\tbench\nVmPeak:\t  900 kB\nVmHWM:\t    300 kB\nVmRSS:\t    200 kB\nThreads:\t12\n";

        assert_eq!(
            (parse_status(text), parse_status("Name:\tbench\n")),
            (
                StatusValues {
                    rss: Some(200 * 1024),
                    peak_rss: Some(300 * 1024),
                    threads: Some(12),
                },
                StatusValues::default()
            )
        );
    }

    #[test]
    fn the_cpu_stat_gives_the_usage_and_the_throttling() {
        let text = "usage_usec 1000\nuser_usec 700\nsystem_usec 300\nnr_periods 40\nnr_throttled 5\nthrottled_usec 2500\nnr_bursts 0\n";

        assert_eq!(
            parse_cpu_stat(text),
            CgroupCpu {
                usage_usec: 1000,
                user_usec: 700,
                system_usec: 300,
                nr_periods: 40,
                nr_throttled: 5,
                throttled_usec: 2500,
            }
        );
    }

    #[test]
    fn the_memory_stat_gives_the_value_of_a_key() {
        let text = "anon 4096\nfile 8192\nfile_mapped 100\n";

        assert_eq!(
            (
                memory_stat_value(text, "anon"),
                memory_stat_value(text, "file"),
                memory_stat_value(text, "shmem")
            ),
            (Some(4096), Some(8192), None)
        );
    }
}
