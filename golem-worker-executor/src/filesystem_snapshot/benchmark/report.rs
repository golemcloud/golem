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

//! The result of one phase of the benchmark, as JSON.
//!
//! A new scenario adds steps with new names, parameters and details. It does not add or change a
//! field of these types.

use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

/// The version of the result format.
pub(super) const FORMAT: &str = "golem-fs-snapshot-benchmark/1";

/// The result of one phase: the work of one pod.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct PhaseResult {
    pub(super) format: &'static str,
    pub(super) run_id: Box<str>,
    pub(super) scenario: &'static str,
    pub(super) phase: &'static str,
    pub(super) tree: &'static str,
    pub(super) cpu_setting: Box<str>,
    pub(super) environment: Value,
    pub(super) volume: Value,
    pub(super) tree_facts: TreeFacts,
    pub(super) steps: Box<[StepRecord]>,
    pub(super) outcome: Outcome,
}

/// What a phase knows about its tree. A field that the phase does not know is `null`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(super) struct TreeFacts {
    pub(super) name: &'static str,
    pub(super) files: Option<u64>,
    pub(super) directories: Option<u64>,
    pub(super) bytes: Option<u64>,
    pub(super) content: Option<&'static str>,
    pub(super) page_cache: Option<&'static str>,
    pub(super) hash: Option<Box<str>>,
    pub(super) hash_after_change: Option<Box<str>>,
    pub(super) change: Option<Value>,
}

/// The outcome of a phase.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Outcome {
    Ok,
    Failed { reason: Box<str> },
}

/// One measured step of a phase.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct StepRecord {
    pub(super) name: &'static str,
    pub(super) status: StepStatus,
    pub(super) parameters: Value,
    pub(super) wall_ms: Option<f64>,
    pub(super) cpu: Option<CpuTime>,
    pub(super) memory: Option<MemoryPeaks>,
    pub(super) threads: Option<ThreadCounts>,
    pub(super) requests: Box<[RequestSummary]>,
    pub(super) bytes_written: u64,
    pub(super) bytes_read: u64,
    pub(super) phases: Box<[PhaseWall]>,
    pub(super) details: Value,
}

impl StepRecord {
    /// Gives the record of a step that did not run, because an earlier step failed.
    pub(super) fn skipped(name: &'static str) -> Self {
        Self {
            name,
            status: StepStatus::Skipped,
            parameters: Value::Object(Default::default()),
            wall_ms: None,
            cpu: None,
            memory: None,
            threads: None,
            requests: Box::default(),
            bytes_written: 0,
            bytes_read: 0,
            phases: Box::default(),
            details: Value::Object(Default::default()),
        }
    }

    /// Gives the record with the details and the phases of the operation.
    pub(super) fn with_details(self, details: Value, phases: Box<[PhaseWall]>) -> Self {
        Self {
            details,
            phases,
            ..self
        }
    }
}

/// Whether a step succeeded.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StepStatus {
    Ok,
    Error(Box<str>),
    Skipped,
}

/// The CPU time of a step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct CpuTime {
    pub(super) user_ms: f64,
    pub(super) system_ms: f64,
    pub(super) cgroup: Option<CgroupCpuTime>,
}

/// The change of the CPU counters of the cgroup of the process during a step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct CgroupCpuTime {
    pub(super) usage_ms: f64,
    pub(super) user_ms: f64,
    pub(super) system_ms: f64,
    pub(super) nr_periods: u64,
    pub(super) nr_throttled: u64,
    pub(super) throttled_ms: f64,
}

/// The memory of a step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct MemoryPeaks {
    pub(super) rss_start_bytes: u64,
    pub(super) rss_peak_bytes: u64,
    pub(super) rss_peak_source: &'static str,
    pub(super) cgroup_current_peak_bytes: Option<u64>,
    pub(super) cgroup_anon_peak_bytes: Option<u64>,
    pub(super) cgroup_file_peak_bytes: Option<u64>,
}

/// The threads of the process during a step. The count includes the thread that samples it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct ThreadCounts {
    pub(super) start: u64,
    pub(super) peak: u64,
    pub(super) sample_interval_ms: u64,
}

/// The blob storage requests of one call and one file type in a step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct RequestSummary {
    pub(super) call: &'static str,
    pub(super) file_type: &'static str,
    pub(super) count: u64,
    pub(super) errors: u64,
    pub(super) bytes: u64,
    pub(super) time_ms: RequestTimes,
}

/// The request times of one call and one file type, by the nearest rank.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct RequestTimes {
    pub(super) min: f64,
    pub(super) p50: f64,
    pub(super) p90: f64,
    pub(super) p99: f64,
    pub(super) max: f64,
    pub(super) total: f64,
}

/// The time of one part of an operation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct PhaseWall {
    pub(super) name: &'static str,
    pub(super) wall_ms: f64,
}

/// Gives the duration in milliseconds, with the precision of a microsecond.
pub(super) fn millis(duration: Duration) -> f64 {
    duration.as_micros() as f64 / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::{
        CgroupCpuTime, CpuTime, FORMAT, MemoryPeaks, Outcome, PhaseResult, PhaseWall,
        RequestSummary, RequestTimes, StepRecord, StepStatus, ThreadCounts, TreeFacts, millis,
    };
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::time::Duration;
    use test_r::test;

    fn result() -> PhaseResult {
        let step = StepRecord {
            name: "cold_save",
            status: StepStatus::Ok,
            parameters: json!({}),
            wall_ms: Some(1500.25),
            cpu: Some(CpuTime {
                user_ms: 900.5,
                system_ms: 100.0,
                cgroup: Some(CgroupCpuTime {
                    usage_ms: 1000.0,
                    user_ms: 900.0,
                    system_ms: 100.0,
                    nr_periods: 15,
                    nr_throttled: 2,
                    throttled_ms: 30.5,
                }),
            }),
            memory: Some(MemoryPeaks {
                rss_start_bytes: 1000,
                rss_peak_bytes: 5000,
                rss_peak_source: "VmHWM",
                cgroup_current_peak_bytes: Some(9000),
                cgroup_anon_peak_bytes: Some(6000),
                cgroup_file_peak_bytes: None,
            }),
            threads: Some(ThreadCounts {
                start: 10,
                peak: 40,
                sample_interval_ms: 10,
            }),
            requests: Box::new([RequestSummary {
                call: "put_raw",
                file_type: "pack",
                count: 3,
                errors: 0,
                bytes: 3000,
                time_ms: RequestTimes {
                    min: 1.0,
                    p50: 2.0,
                    p90: 3.0,
                    p99: 3.0,
                    max: 3.0,
                    total: 6.0,
                },
            }]),
            bytes_written: 3000,
            bytes_read: 0,
            phases: Box::new([PhaseWall {
                name: "backup",
                wall_ms: 1400.0,
            }]),
            details: json!({ "snapshot": "abc" }),
        };
        PhaseResult {
            format: FORMAT,
            run_id: "123-1".into(),
            scenario: "base",
            phase: "save",
            tree: "files-128m",
            cpu_setting: "limit-3".into(),
            environment: json!({ "pod": "p" }),
            volume: json!({ "check": { "status": "ok" } }),
            tree_facts: TreeFacts {
                name: "files-128m",
                files: Some(10_000),
                directories: Some(100),
                bytes: Some(134_217_728),
                content: Some("incompressible"),
                page_cache: Some("dropped"),
                hash: Some("h1".into()),
                hash_after_change: Some("h2".into()),
                change: Some(json!({ "files_rewritten": 10 })),
            },
            steps: Box::new([step, StepRecord::skipped("warm_save")]),
            outcome: Outcome::Failed {
                reason: "the step warm_save failed".into(),
            },
        }
    }

    #[test]
    fn the_result_format_is_stable() {
        assert_eq!(
            serde_json::to_string_pretty(&result()).unwrap(),
            include_str!("golden/phase_result.json").trim_end()
        );
    }

    #[test]
    fn a_duration_is_given_in_milliseconds_with_microseconds() {
        assert_eq!(millis(Duration::from_nanos(1_234_567_890)), 1234.567);
    }
}
