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

//! Reading each executor's own view of what it owns (GOL-364).
//!
//! The shard-manager can say who *should* own what. Only an executor can say
//! what it *believes* it owns, and under a partition those two answers diverge
//! by design — that is the fault, not a bug. What the suite has to rule out is
//! two executors believing the same thing once the partition heals, and that
//! needs every executor's own answer.
//!
//! ### The endpoints file
//!
//! Same boundary as everywhere else in this suite: the driver knows nothing
//! about Kubernetes. It cannot enumerate executor pods, and in cloud mode it has
//! no client for them at all. The workflow opens a forwarded port per executor
//! and describes them in a small JSON file:
//!
//! ```json
//! [{"podName": "worker-executor-abc", "podIp": "10.0.14.207", "endpoint": "localhost:9201"}]
//! ```
//!
//! The file is re-read at every sample rather than loaded once, so a scenario
//! whose executor set changes mid-run — rolling restarts, for one — sees the
//! change without a new mechanism.
//!
//! ### Why the responses are checked against the file
//!
//! Every endpoint is a forwarded port, and a forward crossed at setup would
//! produce a completely self-consistent and completely wrong ownership picture:
//! two entries reporting one executor's shards under two pod names looks exactly
//! like the duplicate-ownership defect the scenario exists to detect. So the
//! snapshot carries the responding executor's own id and the driver compares it
//! against the name the workflow claimed. A mismatch is recorded on the sample,
//! never silently corrected.

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tracing::warn;

/// How long to wait for one executor's introspection response. Short: this is a
/// local forwarded port serving an in-memory read, and a sample that blocks is
/// worse than a sample that records a timeout.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Attempts per executor per sample, and the pause between them.
///
/// Not about the executor — about the hop in front of it. In cloud mode every
/// endpoint is a `kubectl port-forward`, which drops when idle, and the phases
/// of a chaos run are minutes of silence punctuated by one read. The workflow's
/// watchdog restarts a dead forward within a couple of seconds, so a single
/// attempt can fail purely because it was the one that discovered the drop.
///
/// That matters more than it sounds: an executor excluded from a sample is an
/// executor that cannot be compared against the others, and overlap is only
/// visible by comparison. Losing one to a transport blip narrows the verdict
/// for no good reason.
const READ_ATTEMPTS: u32 = 3;
const READ_RETRY_DELAY: Duration = Duration::from_secs(3);

/// One executor the workflow has made reachable, as it described it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorEndpoint {
    /// Kubernetes pod name. The join key between everything the workflow knows
    /// and everything the driver reports.
    pub pod_name: String,
    /// Pod IP, which is how a routing-table entry is matched back to this
    /// executor — the table names pods as `ip:port`.
    pub pod_ip: String,
    /// `host:port` the driver connects to. A forwarded port in cloud mode, and
    /// deliberately not assumed to bear any relation to `pod_ip`.
    pub endpoint: String,
}

/// Reads the endpoints file the workflow maintains.
pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Vec<ExecutorEndpoint>> {
    let raw = std::fs::read_to_string(path.as_ref())
        .with_context(|| format!("reading executor endpoints from {:?}", path.as_ref()))?;
    let endpoints: Vec<ExecutorEndpoint> = serde_json::from_str(&raw)
        .with_context(|| format!("parsing executor endpoints from {:?}", path.as_ref()))?;
    Ok(endpoints)
}

/// What one executor said about itself at one moment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorAssignment {
    /// The pod name the workflow gave for this endpoint.
    pub pod_name: String,
    pub pod_ip: String,
    /// The executor id the response actually carried. Compared against
    /// `pod_name`; see the module docs for why that comparison matters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_executor_id: Option<String>,
    /// Set when the responding executor is not the one the workflow claimed.
    /// A sample carrying this cannot be used for ownership analysis at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_mismatch: Option<String>,
    /// Whether the executor has been assigned anything yet. `false` is a real
    /// answer — a partitioned executor that never registered owns nothing.
    pub assigned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_shards: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shard_ids: Vec<i64>,
    /// Why the executor could not be read. Recorded rather than propagated:
    /// during a partition an unreachable executor is an observation, and losing
    /// the run over it would throw away the evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_error: Option<String>,
}

impl ExecutorAssignment {
    /// Whether this sample can carry weight in the ownership analysis.
    ///
    /// An unreadable executor and a mis-identified one are both excluded, and
    /// both are counted, because a "no overlap" verdict computed over a subset
    /// of the cluster is a weaker claim than it looks.
    pub fn is_usable(&self) -> bool {
        self.read_error.is_none() && self.identity_mismatch.is_none()
    }
}

/// The wire shape of `GET /shard-assignment`, mirroring
/// `golem_worker_executor::shard_introspection::ShardAssignmentSnapshot`.
///
/// Deliberately a separate type rather than a dependency on the executor crate:
/// the driver is a client of a documented HTTP contract, and coupling it to the
/// server's internals would let a breaking change to that contract compile
/// cleanly and fail only on the cluster.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotResponse {
    executor_id: String,
    assigned: bool,
    #[serde(default)]
    number_of_shards: Option<usize>,
    #[serde(default)]
    shard_ids: Vec<i64>,
}

/// Reads every executor's assignment concurrently.
pub async fn sample(endpoints: &[ExecutorEndpoint]) -> Vec<ExecutorAssignment> {
    let client = reqwest::Client::builder()
        .timeout(READ_TIMEOUT)
        .build()
        .unwrap_or_default();

    let mut samples: Vec<ExecutorAssignment> =
        futures::future::join_all(endpoints.iter().map(|endpoint| {
            let client = client.clone();
            async move { read_one(&client, endpoint).await }
        }))
        .await;

    samples.sort_by(|a, b| a.pod_name.cmp(&b.pod_name));
    samples
}

/// Reads one executor, retrying only failures that are about the connection.
///
/// A `404` is not retried: it is a definite answer, and it means the deployed
/// executor does not have the endpoint at all. Retrying that would just spend
/// the sample's budget rediscovering the same fact.
async fn read_one(client: &reqwest::Client, endpoint: &ExecutorEndpoint) -> ExecutorAssignment {
    let mut last = read_once(client, endpoint).await;
    for attempt in 2..=READ_ATTEMPTS {
        let connection_failure = last
            .read_error
            .as_deref()
            .is_some_and(is_connection_failure);
        if !connection_failure {
            break;
        }
        warn!(
            "Chaos: attempt {} of {READ_ATTEMPTS} to read {} failed at the connection; \
             the forward may be restarting",
            attempt - 1,
            endpoint.endpoint
        );
        tokio::time::sleep(READ_RETRY_DELAY).await;
        last = read_once(client, endpoint).await;
    }
    last
}

/// Whether a recorded read error was a failure to reach the endpoint at all, as
/// opposed to the endpoint answering something unusable.
///
/// Keyed on the shape [`read_once`] records rather than on a reqwest type,
/// because by this point the error has already been rendered to a string.
fn is_connection_failure(error: &str) -> bool {
    !error.contains("answered") && !error.contains("unreadable JSON")
}

async fn read_once(client: &reqwest::Client, endpoint: &ExecutorEndpoint) -> ExecutorAssignment {
    let unreadable = |detail: String| ExecutorAssignment {
        pod_name: endpoint.pod_name.clone(),
        pod_ip: endpoint.pod_ip.clone(),
        reported_executor_id: None,
        identity_mismatch: None,
        assigned: false,
        number_of_shards: None,
        shard_ids: Vec::new(),
        read_error: Some(detail),
    };

    let url = format!("http://{}/shard-assignment", endpoint.endpoint);
    let response = match client.get(&url).send().await {
        Ok(response) => response,
        Err(e) => {
            // `{e:#}`-style detail: reqwest's outer message is only "error
            // sending request for url (...)", which does not distinguish a
            // refused connection from a DNS failure or a timeout.
            let detail =
                std::iter::successors(Some(&e as &(dyn std::error::Error + 'static)), |e| {
                    e.source()
                })
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(": ");
            warn!("Chaos: could not read {url}: {detail}");
            return unreadable(detail);
        }
    };
    if !response.status().is_success() {
        return unreadable(format!("{url} answered {}", response.status()));
    }
    let snapshot: SnapshotResponse = match response.json().await {
        Ok(snapshot) => snapshot,
        Err(e) => return unreadable(format!("{url} returned unreadable JSON: {e}")),
    };

    let identity_mismatch = (snapshot.executor_id != endpoint.pod_name).then(|| {
        format!(
            "{} answered for executor {:?}, but the endpoints file names it {:?}",
            endpoint.endpoint, snapshot.executor_id, endpoint.pod_name
        )
    });
    if let Some(mismatch) = &identity_mismatch {
        warn!("Chaos: {mismatch}");
    }

    ExecutorAssignment {
        pod_name: endpoint.pod_name.clone(),
        pod_ip: endpoint.pod_ip.clone(),
        reported_executor_id: Some(snapshot.executor_id),
        identity_mismatch,
        assigned: snapshot.assigned,
        number_of_shards: snapshot.number_of_shards,
        shard_ids: snapshot.shard_ids,
        read_error: None,
    }
}

/// A sample of every executor at one instant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorSample {
    /// Label for when it was taken, e.g. `before-fault`, `after-settle`.
    pub at: String,
    pub taken_at: DateTime<Utc>,
    pub executors: Vec<ExecutorAssignment>,
}

impl ExecutorSample {
    /// Samples every executor listed in the endpoints file *as it stands now*.
    ///
    /// Re-reading the file per sample rather than caching it is what lets a
    /// scenario whose executor set changes mid-run work without a second
    /// mechanism.
    pub async fn take(path: impl AsRef<Path>, at: &str) -> Self {
        let executors = match load(path.as_ref()) {
            Ok(endpoints) => sample(&endpoints).await,
            Err(e) => {
                warn!("Chaos: executor endpoints unavailable at {at}: {e:#}");
                Vec::new()
            }
        };
        Self {
            at: at.to_string(),
            taken_at: Utc::now(),
            executors,
        }
    }

    pub fn usable(&self) -> impl Iterator<Item = &ExecutorAssignment> {
        self.executors.iter().filter(|e| e.is_usable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn endpoints_file(contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "golem-chaos-executors-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn endpoints_parse_from_the_shape_the_workflow_writes() {
        let path = endpoints_file(
            r#"[
                {"podName": "worker-executor-a", "podIp": "10.0.1.1", "endpoint": "localhost:9201"},
                {"podName": "worker-executor-b", "podIp": "10.0.1.2", "endpoint": "localhost:9202"}
            ]"#,
        );
        let endpoints = load(&path).unwrap();
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].pod_name, "worker-executor-a");
        assert_eq!(endpoints[1].endpoint, "localhost:9202");
    }

    /// A missing or malformed file must not take the run down: the sample is
    /// recorded empty and the analysis reports that it had nothing to work with.
    #[test]
    async fn a_missing_endpoints_file_yields_an_empty_sample_rather_than_an_error() {
        let sample = ExecutorSample::take("/nonexistent/executors.json", "before-fault").await;
        assert_eq!(sample.at, "before-fault");
        assert!(sample.executors.is_empty());
    }

    /// An unreadable executor is an observation, not a failure — during a
    /// partition it is the expected observation.
    #[test]
    fn an_unreadable_executor_is_excluded_from_analysis_but_kept_in_the_sample() {
        let unreadable = ExecutorAssignment {
            pod_name: "worker-executor-a".to_string(),
            read_error: Some("connection refused".to_string()),
            ..Default::default()
        };
        assert!(!unreadable.is_usable());
    }

    /// The retry exists for the hop in front of the executor, not the executor
    /// itself. An endpoint that *answered* has given a definite answer, and
    /// retrying it would just spend the sample's budget rediscovering it — a
    /// `404` from a cluster running an older image being the case that matters.
    #[test]
    fn only_connection_failures_are_worth_retrying() {
        assert!(is_connection_failure(
            "error sending request for url (http://localhost:9301/shard-assignment):              tcp connect error: Connection refused (os error 111)"
        ));
        assert!(is_connection_failure("operation timed out"));
        assert!(!is_connection_failure(
            "http://localhost:9302/shard-assignment answered 404 Not Found"
        ));
        assert!(!is_connection_failure(
            "http://localhost:9302/shard-assignment answered 503 Service Unavailable"
        ));
        assert!(!is_connection_failure(
            "http://localhost:9302/shard-assignment returned unreadable JSON: expected value"
        ));
    }

    /// A crossed port-forward produces a perfectly plausible and completely
    /// wrong ownership picture, so a mis-identified response is excluded too.
    #[test]
    fn a_mis_identified_executor_is_excluded_from_analysis() {
        let crossed = ExecutorAssignment {
            pod_name: "worker-executor-a".to_string(),
            reported_executor_id: Some("worker-executor-b".to_string()),
            identity_mismatch: Some("crossed forward".to_string()),
            assigned: true,
            shard_ids: vec![1, 2, 3],
            ..Default::default()
        };
        assert!(!crossed.is_usable());
    }
}
