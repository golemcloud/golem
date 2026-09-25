use super::{BenchmarkRecorder, ResultKey};
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use anyhow::{Context, bail, ensure};
use std::collections::BTreeMap;
use std::time::Duration;

const METRIC: &str = "golem_storage_logical_operations_total";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorageOperation {
    pub kind: String,
    pub operation: String,
    pub service: String,
    pub api: String,
    pub entity: String,
}

#[derive(Debug, Clone)]
struct ExecutorSnapshot {
    generation: u64,
    counters: BTreeMap<StorageOperation, u64>,
}

#[derive(Debug, Clone, Default)]
pub struct StorageSnapshot {
    executors: BTreeMap<String, ExecutorSnapshot>,
}

impl StorageSnapshot {
    /// Missing baseline (or a changed spawned generation) means counts since
    /// process start. Disappearing members/series and same-generation decreases
    /// fail closed instead of looking like a zero-operation phase.
    pub fn delta(
        &self,
        previous: Option<&Self>,
    ) -> anyhow::Result<BTreeMap<StorageOperation, u64>> {
        let mut delta = BTreeMap::new();
        if let Some(previous) = previous {
            ensure!(
                previous.executors.keys().eq(self.executors.keys()),
                "metrics cluster membership changed"
            );
        }
        for (endpoint, current) in &self.executors {
            let before = previous
                .and_then(|snapshot| snapshot.executors.get(endpoint))
                .filter(|snapshot| snapshot.generation == current.generation);
            if let Some(before) = before {
                ensure!(
                    before
                        .counters
                        .keys()
                        .all(|key| current.counters.contains_key(key)),
                    "storage counter disappeared at {endpoint}"
                );
            }
            for (key, value) in &current.counters {
                let old = before
                    .and_then(|snapshot| snapshot.counters.get(key))
                    .copied()
                    .unwrap_or(0);
                let count = value
                    .checked_sub(old)
                    .context("storage counter decreased without a process restart")?;
                let total = delta.entry(key.clone()).or_insert(0u64);
                *total = total
                    .checked_add(count)
                    .context("storage counter sum overflow")?;
            }
        }
        Ok(delta)
    }

    /// Explicit selections retain zero calls too. Names must include enough
    /// labels to identify the selected operation; scrapes are coarse windows,
    /// not proof of foreground/background causal ownership.
    pub fn record_delta(
        &self,
        previous: Option<&Self>,
        recorder: &BenchmarkRecorder,
        selected: &[(StorageOperation, ResultKey)],
    ) -> anyhow::Result<()> {
        let delta = self.delta(previous)?;
        for (operation, key) in selected {
            recorder.count(key, delta.get(operation).copied().unwrap_or(0));
        }
        Ok(())
    }
}

pub struct StorageMetricsClient {
    client: reqwest::Client,
}

impl StorageMetricsClient {
    pub fn new(timeout: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder().timeout(timeout).build()?,
        })
    }

    /// Every member must scrape successfully. Call outside the timed protocol
    /// reader, and take the first snapshot after restart as a fresh baseline.
    pub async fn snapshot(
        &self,
        cluster: &dyn WorkerExecutorCluster,
    ) -> anyhow::Result<StorageSnapshot> {
        let executors = cluster.to_vec();
        ensure!(
            !executors.is_empty(),
            "storage metrics require spawned executors"
        );
        let mut snapshot = StorageSnapshot::default();
        for executor in executors {
            let (url, generation) = executor
                .metrics_endpoint()
                .context("storage metrics require spawned executors")?;
            let body = self
                .client
                .get(&url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            ensure!(
                executor.metrics_endpoint() == Some((url.clone(), generation)),
                "executor restarted during metrics scrape"
            );
            ensure!(
                snapshot
                    .executors
                    .insert(
                        url,
                        ExecutorSnapshot {
                            generation,
                            counters: parse_counters(&body)?
                        }
                    )
                    .is_none(),
                "duplicate executor metrics endpoint"
            );
        }
        Ok(snapshot)
    }
}

fn parse_counters(body: &str) -> anyhow::Result<BTreeMap<StorageOperation, u64>> {
    ensure!(
        body.lines()
            .any(|line| line.starts_with("executor_version_info{")),
        "scrape is not a complete executor metrics response"
    );
    let mut counters = BTreeMap::new();
    for line in body.lines().filter(|line| line.starts_with(METRIC)) {
        let suffix = &line[METRIC.len()..];
        ensure!(suffix.starts_with('{'), "invalid storage counter sample");
        let end = suffix
            .rfind('}')
            .context("unterminated storage counter labels")?;
        let mut labels = BTreeMap::new();
        // These labels are static source-code identifiers, never keys or paths.
        for pair in suffix[1..end].split(',') {
            let (key, value) = pair.split_once('=').context("invalid counter label")?;
            ensure!(
                labels
                    .insert(key, serde_json::from_str::<String>(value)?)
                    .is_none(),
                "duplicate counter label"
            );
        }
        let mut take = |name| {
            labels
                .remove(name)
                .with_context(|| format!("missing {name} label"))
        };
        let key = StorageOperation {
            kind: take("kind")?,
            operation: take("operation")?,
            service: take("service")?,
            api: take("api")?,
            entity: take("entity")?,
        };
        ensure!(labels.is_empty(), "unexpected storage counter labels");
        ensure!(
            matches!(key.kind.as_str(), "indexed" | "keyvalue" | "blob"),
            "unknown storage kind"
        );
        let count = suffix[end + 1..]
            .trim()
            .parse::<u64>()
            .context("storage counter is not an integer")?;
        if counters.insert(key, count).is_some() {
            bail!("duplicate storage counter sample");
        }
    }
    Ok(counters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn snapshot(generation: u64, count: u64) -> StorageSnapshot {
        let body = format!(
            "executor_version_info{{version=\"test\"}} 1\n{METRIC}{{kind=\"keyvalue\",operation=\"get_many\",service=\"stream_session_index\",api=\"read_recovery\",entity=\"metadata\"}} {count}\n"
        );
        StorageSnapshot {
            executors: BTreeMap::from([(
                "executor-a".into(),
                ExecutorSnapshot {
                    generation,
                    counters: parse_counters(&body).unwrap(),
                },
            )]),
        }
    }

    #[test]
    fn storage_metrics_deltas_handle_restarts_and_reject_missing_data() {
        assert!(parse_counters("").is_err());
        assert!(parse_counters("<html>proxy error</html>").is_err());
        assert!(
            parse_counters("executor_version_info{version=\"test\"} 1\n")
                .unwrap()
                .is_empty()
        );
        let before = snapshot(0, 100);
        let after = snapshot(0, 107);
        assert_eq!(
            after
                .delta(Some(&before))
                .unwrap()
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![7]
        );
        let restarted = snapshot(1, 3);
        assert_eq!(
            restarted
                .delta(Some(&after))
                .unwrap()
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert!(snapshot(0, 99).delta(Some(&before)).is_err());
        assert!(StorageSnapshot::default().delta(Some(&before)).is_err());
        let mut missing = after.clone();
        missing
            .executors
            .get_mut("executor-a")
            .unwrap()
            .counters
            .clear();
        assert!(missing.delta(Some(&before)).is_err());
        let mut cluster = after.clone();
        cluster.executors.insert(
            "executor-b".into(),
            snapshot(0, 11).executors.remove("executor-a").unwrap(),
        );
        assert_eq!(
            cluster
                .delta(None)
                .unwrap()
                .values()
                .copied()
                .collect::<Vec<_>>(),
            vec![118]
        );
        let operation = after.delta(None).unwrap().into_keys().next().unwrap();
        assert_eq!(operation.api, "read_recovery");
        let recorder = BenchmarkRecorder::new();
        after
            .record_delta(
                Some(&after),
                &recorder,
                &[(
                    operation,
                    ResultKey::primary("storage-idle-keyvalue-get_many"),
                )],
            )
            .unwrap();
        assert_eq!(recorder.counts().values().next().unwrap(), &vec![0]);
    }
}
