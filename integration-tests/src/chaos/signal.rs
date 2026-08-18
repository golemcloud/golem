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

//! The workflow ↔ driver fault-signal contract (GOL-363).
//!
//! The driver never injects a fault itself and never talks to Kubernetes. The
//! workflow owns the Chaos Mesh custom resource; the two sides meet at a
//! directory of small JSON files:
//!
//! | File | Written by | Meaning |
//! | -- | -- | -- |
//! | `baseline-ready.json` | driver | Baseline workload is at steady state — safe to inject |
//! | `fault-injected.json` | workflow | The fault is applied *and verified active* |
//! | `fault-recovered.json` | workflow | The fault is removed and the target is healthy again |
//!
//! Keeping the driver on this side of the line is what lets the same scenario
//! code run against a local cluster, a different orchestrator, or a hand-driven
//! shell session: walking a scenario through its phases needs nothing but
//! `echo '{...}' > fault-injected.json`.
//!
//! Every wait is bounded. A signal that never arrives is a scenario abort with
//! whatever artifacts the run produced, never a hang — a stuck driver would hold
//! the `golem-dev` maintenance window open with nothing to show for it.

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::Instant;
use tracing::{info, warn};

/// File name the driver writes once its baseline workload is at steady state.
pub const BASELINE_READY_FILE: &str = "baseline-ready.json";
/// File name the workflow writes once the fault is applied and verified active.
pub const FAULT_INJECTED_FILE: &str = "fault-injected.json";
/// File name the workflow writes once the fault is removed and the target is
/// healthy again.
pub const FAULT_RECOVERED_FILE: &str = "fault-recovered.json";

/// How often a wait re-checks for its file. The signals are minutes apart, so
/// this is about keeping the abort responsive, not about latency.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Where the driver determined the fault has to land, for scenarios that need a
/// *specific* pod rather than any pod of a deployment (GOL-366).
///
/// Chaos Mesh's `mode: one` picks a pod at random, which is the right thing when
/// the experiment is "lose a shard-manager" and the wrong thing when it is "lose
/// the executor that is currently running these fifty invocations". Only the
/// driver can know the second: it holds the routing table and can compute which
/// executor owns which agent. So it names the target and the workflow aims at
/// it.
///
/// The driver still knows nothing about Kubernetes. It reports the endpoint the
/// *routing table* uses — an `ip:port` — and resolving that to a pod name is the
/// workflow's job, the same way turning phase windows into Grafana links is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultTarget {
    /// The executor endpoint as the shard-manager names it, e.g.
    /// `10.0.14.207:9000`.
    pub pod_address: String,
    /// The address's host part, split out because that is what a Kubernetes
    /// `status.podIP` field selector matches — and a workflow doing the split
    /// in shell would be one more place to get it wrong.
    pub pod_ip: String,
    /// The agents this pod was verified to own, in the order the scenario will
    /// drive them. Recorded so the artifact can be read back later and the
    /// claim re-checked rather than taken on trust.
    pub owned_agents: Vec<String>,
}

/// Written by the driver: the baseline workload has run long enough to be at
/// steady state, so injecting now measures recovery rather than warm-up.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineReady {
    pub scenario_code: String,
    pub ready_at: DateTime<Utc>,
    /// Operations the baseline phase completed. Purely informational — it lets
    /// the workflow log something meaningful before it injects.
    pub baseline_operations: u64,
    /// Present only for scenarios that pin the fault to one pod. Absent means
    /// "any pod of the configured target will do", which is what S12 wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault_target: Option<FaultTarget>,
}

/// Written by the workflow: the fault is applied **and confirmed active**. The
/// confirmation matters — injecting and immediately reporting success would let
/// a scenario record a recovery it never actually forced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultInjected {
    /// Identifier of the fault resource, echoed into the result so an operator
    /// can correlate the run with what the cluster was asked to do.
    pub fault_id: String,
    /// Fault kind as the workflow named it, e.g. `pod-kill`.
    pub kind: String,
    /// What the fault was aimed at, e.g. `shard-manager`.
    pub target: String,
    pub injected_at: DateTime<Utc>,
}

/// Written by the workflow: the fault is gone and the target has rolled back to
/// healthy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultRecovered {
    pub recovered_at: DateTime<Utc>,
    /// How the fault ended, e.g. `deleted` for the normal path or
    /// `duration_expired` when Chaos Mesh retired it on its own.
    pub termination_reason: String,
}

/// Why a wait gave up.
#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    #[error("timed out after {waited:?} waiting for {file} in {dir}")]
    Timeout {
        file: &'static str,
        dir: String,
        waited: Duration,
    },
    #[error("{file} in {dir} is not valid signal JSON: {source}")]
    Malformed {
        file: &'static str,
        dir: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Io(#[from] anyhow::Error),
}

/// The driver's handle on the signal directory.
#[derive(Debug, Clone)]
pub struct FaultSignals {
    dir: PathBuf,
}

impl FaultSignals {
    /// Opens (creating if needed) the signal directory.
    pub fn new(dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating signal directory {dir:?}"))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Announces that the baseline phase is done and the fault may be injected.
    pub fn write_baseline_ready(&self, signal: &BaselineReady) -> anyhow::Result<()> {
        self.write(BASELINE_READY_FILE, signal)
    }

    /// Blocks until the workflow reports the fault active, or `timeout` elapses.
    pub async fn await_fault_injected(
        &self,
        timeout: Duration,
    ) -> Result<FaultInjected, SignalError> {
        self.await_file(FAULT_INJECTED_FILE, timeout).await
    }

    /// Blocks until the workflow reports the fault cleared, or `timeout` elapses.
    pub async fn await_fault_recovered(
        &self,
        timeout: Duration,
    ) -> Result<FaultRecovered, SignalError> {
        self.await_file(FAULT_RECOVERED_FILE, timeout).await
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> anyhow::Result<()> {
        let path = self.dir.join(name);
        // Write-then-rename: the workflow polls for existence, so a half-written
        // file must never be visible under the name it polls for.
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let json = serde_json::to_string_pretty(value)?;
        std::fs::write(&tmp, json).with_context(|| format!("writing signal {tmp:?}"))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("publishing signal {tmp:?} -> {path:?}"))?;
        info!("Wrote chaos signal {path:?}");
        Ok(())
    }

    async fn await_file<T: for<'de> Deserialize<'de>>(
        &self,
        name: &'static str,
        timeout: Duration,
    ) -> Result<T, SignalError> {
        let path = self.dir.join(name);
        let started = Instant::now();
        info!("Waiting up to {timeout:?} for chaos signal {path:?} (written by the workflow)");

        loop {
            match std::fs::read_to_string(&path) {
                Ok(raw) => {
                    // A reader can still land between create and rename on
                    // filesystems where rename is not atomic; an empty or
                    // truncated read is retried rather than treated as
                    // malformed, up to the same deadline.
                    if !raw.trim().is_empty() {
                        return serde_json::from_str(&raw).map_err(|source| {
                            SignalError::Malformed {
                                file: name,
                                dir: self.dir.display().to_string(),
                                source,
                            }
                        });
                    }
                    warn!("Chaos signal {path:?} is empty, still waiting");
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(SignalError::Io(
                        anyhow::Error::new(e).context(format!("reading chaos signal {path:?}")),
                    ));
                }
            }

            let waited = started.elapsed();
            if waited >= timeout {
                return Err(SignalError::Timeout {
                    file: name,
                    dir: self.dir.display().to_string(),
                    waited,
                });
            }
            tokio::time::sleep(POLL_INTERVAL.min(timeout - waited)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn temp_signal_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "golem-chaos-signal-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn injected(at: DateTime<Utc>) -> FaultInjected {
        FaultInjected {
            fault_id: "s12-pod-kill".to_string(),
            kind: "pod-kill".to_string(),
            target: "shard-manager".to_string(),
            injected_at: at,
        }
    }

    #[test]
    async fn await_returns_a_signal_written_before_the_wait() {
        let dir = temp_signal_dir("pre-written");
        let signals = FaultSignals::new(&dir).unwrap();
        let at = Utc::now();
        std::fs::write(
            dir.join(FAULT_INJECTED_FILE),
            serde_json::to_string(&injected(at)).unwrap(),
        )
        .unwrap();

        let got = signals
            .await_fault_injected(Duration::from_secs(5))
            .await
            .expect("signal already present");
        assert_eq!(got.kind, "pod-kill");
        assert_eq!(got.target, "shard-manager");
    }

    #[test]
    async fn await_returns_a_signal_written_during_the_wait() {
        let dir = temp_signal_dir("during-wait");
        let signals = FaultSignals::new(&dir).unwrap();
        let write_dir = dir.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            std::fs::write(
                write_dir.join(FAULT_RECOVERED_FILE),
                serde_json::to_string(&FaultRecovered {
                    recovered_at: Utc::now(),
                    termination_reason: "deleted".to_string(),
                })
                .unwrap(),
            )
            .unwrap();
        });

        let got = signals
            .await_fault_recovered(Duration::from_secs(10))
            .await
            .expect("signal arrives mid-wait");
        assert_eq!(got.termination_reason, "deleted");
    }

    /// A signal that never arrives must abort the scenario rather than hold the
    /// maintenance window open forever.
    #[test]
    async fn await_times_out_when_no_signal_arrives() {
        let dir = temp_signal_dir("timeout");
        let signals = FaultSignals::new(&dir).unwrap();

        let err = signals
            .await_fault_injected(Duration::from_millis(300))
            .await
            .expect_err("no signal was ever written");
        assert!(
            matches!(err, SignalError::Timeout { file, .. } if file == FAULT_INJECTED_FILE),
            "expected a timeout naming the awaited file, got {err:?}"
        );
    }

    /// The workflow aims a pinned fault using exactly these fields, so they have
    /// to survive the round trip through the signal file under the names the
    /// workflow's `jq` expressions use.
    #[test]
    async fn a_pinned_fault_target_round_trips_under_its_json_names() {
        let dir = temp_signal_dir("fault-target");
        let signals = FaultSignals::new(&dir).unwrap();
        signals
            .write_baseline_ready(&BaselineReady {
                scenario_code: "S8".to_string(),
                ready_at: Utc::now(),
                baseline_operations: 50,
                fault_target: Some(FaultTarget {
                    pod_address: "10.0.14.207:9000".to_string(),
                    pod_ip: "10.0.14.207".to_string(),
                    owned_agents: vec!["chaos-s8-pinned-http-0000".to_string()],
                }),
            })
            .unwrap();

        let raw = std::fs::read_to_string(dir.join(BASELINE_READY_FILE)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json["faultTarget"]["podIp"], "10.0.14.207");
        assert_eq!(json["faultTarget"]["podAddress"], "10.0.14.207:9000");
    }

    /// S12 does not pin its fault, and its signal must stay free of the field
    /// rather than carrying a null the workflow has to special-case.
    #[test]
    async fn an_unpinned_baseline_ready_omits_the_fault_target_entirely() {
        let dir = temp_signal_dir("no-fault-target");
        let signals = FaultSignals::new(&dir).unwrap();
        signals
            .write_baseline_ready(&BaselineReady {
                scenario_code: "S12".to_string(),
                ready_at: Utc::now(),
                baseline_operations: 7,
                fault_target: None,
            })
            .unwrap();

        let raw = std::fs::read_to_string(dir.join(BASELINE_READY_FILE)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json.get("faultTarget").is_none());
    }

    /// Garbage in the signal file is reported as such, not silently retried
    /// until the timeout — the operator needs to see the real cause.
    #[test]
    async fn await_reports_malformed_signal_json() {
        let dir = temp_signal_dir("malformed");
        let signals = FaultSignals::new(&dir).unwrap();
        std::fs::write(dir.join(FAULT_INJECTED_FILE), "{not json").unwrap();

        let err = signals
            .await_fault_injected(Duration::from_secs(5))
            .await
            .expect_err("malformed JSON must not be accepted");
        assert!(
            matches!(err, SignalError::Malformed { .. }),
            "expected a malformed-signal error, got {err:?}"
        );
    }

    /// The workflow polls for the file by name, so a partially written file must
    /// never be visible under that name.
    #[test]
    async fn baseline_ready_is_published_atomically_and_round_trips() {
        let dir = temp_signal_dir("baseline");
        let signals = FaultSignals::new(&dir).unwrap();
        let ready = BaselineReady {
            scenario_code: "S12".to_string(),
            ready_at: Utc::now(),
            baseline_operations: 1234,
            fault_target: None,
        };
        signals.write_baseline_ready(&ready).unwrap();

        let raw = std::fs::read_to_string(dir.join(BASELINE_READY_FILE)).unwrap();
        let parsed: BaselineReady = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.scenario_code, "S12");
        assert_eq!(parsed.baseline_operations, 1234);

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp file should have been renamed away, found {leftovers:?}"
        );
    }
}
