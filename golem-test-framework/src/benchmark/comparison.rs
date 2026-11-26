// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::{BenchmarkSuiteResult, RunConfig};
use anyhow::{Result, ensure};
use rand::{Rng, SeedableRng, rngs::StdRng};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
pub struct BenchmarkComparison {
    pub base_source: String,
    pub head_source: String,
    pub process_pairs: usize,
    pub interval_method: &'static str,
    pub results: Vec<MetricComparison>,
}

#[derive(Debug, Serialize)]
pub struct MetricComparison {
    pub entry: String,
    pub run_config: RunConfig,
    pub metric: String,
    pub entry_version: u32,
    pub series_key: String,
    pub base_observations: Vec<Option<f64>>,
    pub head_observations: Vec<Option<f64>>,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_change: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_interval: Option<[f64; 2]>,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct MeasurementKey {
    entry: String,
    config: String,
    metric: String,
    version: u32,
    series: String,
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        // Nonnegative observations and relative changes >= -1 avoid overflow here.
        values[mid - 1] / 2.0 + values[mid] / 2.0
    }
}

fn measurements(run: &BenchmarkSuiteResult) -> Result<BTreeMap<MeasurementKey, Option<f64>>> {
    let mut values = BTreeMap::new();
    let mut configurations = BTreeSet::new();
    for entry in &run.results {
        ensure!(
            entry.entry_version > 0
                && entry.iterations > 0
                && !entry.measurement_environment.runner_id.is_empty()
                && !entry
                    .measurement_environment
                    .hardware_fingerprint
                    .is_empty()
                && !entry.measurement_environment.storage_profile.is_empty()
                && entry.series_key
                    == entry
                        .measurement_environment
                        .series_key(run.harness_version, entry.entry_version),
            "invalid series identity for {}",
            entry.name
        );
        ensure!(
            run.runner
                .as_ref()
                .is_some_and(|runner| runner.id == entry.measurement_environment.runner_id),
            "entry runner mismatch"
        );
        ensure!(
            entry.runs.len() == entry.results.len(),
            "missing entry configurations"
        );
        let declared: BTreeSet<_> = entry
            .runs
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()?;
        let observed: BTreeSet<_> = entry
            .results
            .iter()
            .map(|result| serde_json::to_string(&result.run_config))
            .collect::<Result<_, _>>()?;
        ensure!(
            declared == observed && observed.len() == entry.results.len(),
            "duplicate or missing entry configurations"
        );
        for result in &entry.results {
            let config = serde_json::to_string(&result.run_config)?;
            ensure!(
                configurations.insert((entry.name.clone(), config.clone())),
                "duplicate entry configuration"
            );
            ensure!(
                result.failure_count() == 0
                    && result.iteration_results.len() == entry.iterations
                    && result
                        .iteration_results
                        .iter()
                        .enumerate()
                        .all(|(index, item)| item.index == index
                            && item.skipped.is_none()
                            && item.failures.is_empty()),
                "failed or incomplete iterations for {}",
                entry.name
            );
            let keys: BTreeSet<_> = result
                .iteration_results
                .iter()
                .flat_map(|item| item.duration_samples.keys())
                .filter(|key| key.is_primary())
                .collect();
            for metric in keys {
                let mut iterations = Vec::new();
                for item in &result.iteration_results {
                    if let Some(samples) = item
                        .duration_samples
                        .get(metric)
                        .filter(|samples| !samples.is_empty())
                    {
                        ensure!(
                            samples
                                .iter()
                                .all(|value| value.is_finite() && *value > 0.0),
                            "invalid duration for {metric}"
                        );
                        iterations.push(median(&mut samples.clone()));
                    }
                }
                let value = (iterations.len() == entry.iterations).then(|| median(&mut iterations));
                values.insert(
                    MeasurementKey {
                        entry: entry.name.clone(),
                        config: config.clone(),
                        metric: metric.to_string(),
                        version: entry.entry_version,
                        series: entry.series_key.clone(),
                    },
                    value,
                );
            }
        }
    }
    Ok(values)
}

impl BenchmarkComparison {
    /// Each base/head pair is one observation, not each request or shared-context iteration.
    pub fn from_pairs(
        base: &[BenchmarkSuiteResult],
        head: &[BenchmarkSuiteResult],
    ) -> Result<Self> {
        ensure!(
            base.len() >= 3 && base.len() == head.len(),
            "supply at least three complete process pairs"
        );
        let first = &base[0];
        let source = |run: &BenchmarkSuiteResult| -> Result<String> {
            run.source
                .as_ref()
                .and_then(|source| source.commit_sha.clone())
                .filter(|sha| !sha.is_empty())
                .ok_or_else(|| anyhow::anyhow!("missing source commit"))
        };
        let base_source = source(first)?;
        let head_source = source(&head[0])?;
        let fingerprint = first
            .suite_fingerprint
            .as_ref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing suite fingerprint"))?;
        let artifact = first
            .artifacts
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing harness artifact"))?;
        ensure!(
            !artifact.runner.sha256.is_empty()
                && first
                    .runner
                    .as_ref()
                    .is_some_and(|runner| !runner.id.is_empty()),
            "missing runner identity"
        );
        let mut ids = BTreeSet::new();
        for (runs, expected) in [(base, &base_source), (head, &head_source)] {
            for run in runs {
                ensure!(
                    run.schema_version == 2
                        && run.harness_version == first.harness_version
                        && run.suite == first.suite
                        && run.suite_fingerprint.as_ref() == Some(fingerprint)
                        && run.runner == first.runner
                        && run.artifacts.as_ref().is_some_and(
                            |artifacts| artifacts.runner.sha256 == artifact.runner.sha256
                        ),
                    "comparison requires the same suite, runner and harness"
                );
                ensure!(
                    source(run)? == *expected
                        && run
                            .source
                            .as_ref()
                            .and_then(|source| source.repository.as_ref())
                            == first
                                .source
                                .as_ref()
                                .and_then(|source| source.repository.as_ref()),
                    "source mismatch"
                );
                let id = run
                    .run_id
                    .as_ref()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("missing process run ID"))?;
                ensure!(ids.insert(id), "reused process result");
                ensure!(
                    run.artifacts == runs[0].artifacts,
                    "runtime artifacts changed within one side"
                );
            }
        }
        let base: Vec<_> = base.iter().map(measurements).collect::<Result<_>>()?;
        let head: Vec<_> = head.iter().map(measurements).collect::<Result<_>>()?;
        let keys: BTreeSet<_> = base
            .iter()
            .chain(&head)
            .flat_map(|values| values.keys())
            .collect();
        ensure!(!keys.is_empty(), "no primary duration measurements");
        let mut results = Vec::new();
        for key in keys {
            let observations = |runs: &[BTreeMap<MeasurementKey, Option<f64>>]| {
                runs.iter()
                    .map(|run| run.get(key).copied().flatten())
                    .collect::<Vec<_>>()
            };
            let base_observations = observations(&base);
            let head_observations = observations(&head);
            let mut result = MetricComparison {
                entry: key.entry.clone(),
                run_config: serde_json::from_str(&key.config)?,
                metric: key.metric.clone(),
                entry_version: key.version,
                series_key: key.series.clone(),
                base_observations,
                head_observations,
                status: "incomplete-pairs",
                relative_change: None,
                confidence_interval: None,
            };
            let changes: Option<Vec<f64>> = result
                .base_observations
                .iter()
                .zip(&result.head_observations)
                .map(|(base, head)| Some(head.as_ref()? / base.as_ref()? - 1.0))
                .collect();
            if let Some(mut changes) = changes {
                if changes.iter().all(|value| value.is_finite()) {
                    let mut rng = StdRng::seed_from_u64(0);
                    let mut bootstrap: Vec<_> = (0..10_000)
                        .map(|_| {
                            let mut sample: Vec<_> = (0..changes.len())
                                .map(|_| changes[rng.random_range(0..changes.len())])
                                .collect();
                            median(&mut sample)
                        })
                        .collect();
                    bootstrap.sort_by(f64::total_cmp);
                    let interval = [bootstrap[249], bootstrap[9749]];
                    result.relative_change = Some(median(&mut changes));
                    result.confidence_interval = Some(interval);
                    result.status = if interval[0] > 0.0 {
                        "regression"
                    } else if interval[1] < 0.0 {
                        "improvement"
                    } else {
                        "inconclusive"
                    };
                } else {
                    result.status = "unrepresentable-relative-change";
                }
            }
            results.push(result);
        }
        Ok(Self {
            base_source,
            head_source,
            process_pairs: base.len(),
            interval_method: "descriptive 95% paired percentile bootstrap; 10000 resamples; seed 0; median of process-pair relative changes, each process is the median of iteration medians",
            results,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::{
        BenchmarkArtifact, BenchmarkArtifacts, BenchmarkRecorder, BenchmarkResult,
        BenchmarkRunResult, BenchmarkRunner, BenchmarkSource, MeasurementEnvironment, ResultKey,
    };
    use std::time::Duration;
    use test_r::test;

    fn run(source: &str, index: usize, value: f64, first_factor: f64) -> BenchmarkSuiteResult {
        let config = RunConfig {
            cluster_size: 1,
            size: 7,
            length: 13,
            disable_compilation_cache: false,
        };
        let environment = MeasurementEnvironment {
            runner_id: "runner".into(),
            hardware_fingerprint: "hardware".into(),
            storage_profile: "postgres".into(),
            sockets_enabled: false,
        };
        let mut result = BenchmarkRunResult::new(config.clone());
        for (index, (factor, count)) in [(first_factor, 101), (1.0, 1), (0.5, 1)]
            .into_iter()
            .enumerate()
        {
            let recorder = BenchmarkRecorder::new();
            for _ in 0..count {
                recorder.duration(
                    &ResultKey::primary("latency"),
                    Duration::from_secs_f64(value * factor / 1000.0),
                );
            }
            recorder.count(&ResultKey::primary("calls"), count);
            result.add_iteration(index, recorder);
        }
        let mut run = BenchmarkSuiteResult::new("comparison");
        run.runner = Some(BenchmarkRunner {
            id: "runner".into(),
            label: None,
        });
        run.source = Some(BenchmarkSource {
            repository: Some("golemcloud/golem".into()),
            commit_sha: Some(source.into()),
            source_ref: None,
        });
        run.run_id = Some(format!("{source}-{index}"));
        run.suite_fingerprint = Some("suite".into());
        run.artifacts = Some(BenchmarkArtifacts {
            runner: BenchmarkArtifact {
                path: "harness".into(),
                sha256: "harness-sha".into(),
            },
            services: BTreeMap::new(),
            fixtures: BTreeMap::new(),
        });
        run.add(BenchmarkResult {
            name: "asymmetric".into(),
            entry_version: 1,
            iterations: 3,
            series_key: environment.series_key(run.harness_version, 1),
            measurement_environment: environment,
            ceiling: None,
            description: "test".into(),
            runs: vec![config],
            results: vec![result],
            run_id: None,
        });
        run
    }

    fn pairs() -> (Vec<BenchmarkSuiteResult>, Vec<BenchmarkSuiteResult>) {
        (
            [10.0, 100.0, 1000.0]
                .into_iter()
                .enumerate()
                .map(|(i, value)| run("base", i, value, 5.0))
                .collect(),
            [9.0, 200.0, 1100.0]
                .into_iter()
                .enumerate()
                .map(|(i, value)| run("head", i, value, 2.0))
                .collect(),
        )
    }

    #[test]
    fn paired_comparison_uses_iteration_medians_and_keeps_process_pairs() {
        let (base, head) = pairs();
        let report = BenchmarkComparison::from_pairs(&base, &head).unwrap();
        assert_eq!(report.process_pairs, 3);
        assert_eq!(report.results.len(), 1);
        let metric = &report.results[0];
        assert_eq!(
            metric.base_observations,
            vec![Some(10.0), Some(100.0), Some(1000.0)]
        );
        assert_eq!(
            metric.head_observations,
            vec![Some(9.0), Some(200.0), Some(1100.0)]
        );
        assert!((metric.relative_change.unwrap() - 0.1).abs() < 1e-12);
        assert_eq!(metric.status, "inconclusive");
        let interval = metric.confidence_interval.unwrap();
        assert!((interval[0] + 0.1).abs() < 1e-12 && (interval[1] - 1.0).abs() < 1e-12);
        let improved: Vec<_> = (0..3)
            .map(|i| run("head", i, [5.0, 50.0, 500.0][i], 2.0))
            .collect();
        assert_eq!(
            BenchmarkComparison::from_pairs(&base, &improved)
                .unwrap()
                .results[0]
                .status,
            "improvement"
        );
    }

    #[test]
    fn paired_comparison_rejects_failed_reused_or_mixed_evidence() {
        let (base, head) = pairs();
        for mutation in 0..8 {
            let mut wrong = head.clone();
            match mutation {
                0 => wrong[1].run_id = wrong[0].run_id.clone(),
                1 => wrong[1].runner.as_mut().unwrap().id = "other".into(),
                2 => wrong[1].results[0].series_key = "invented".into(),
                3 => wrong[1].artifacts.as_mut().unwrap().runner.sha256 = "other".into(),
                4 => {
                    wrong[1].results[0].results[0].iteration_results.pop();
                }
                5 => {
                    wrong[1].results[0].results[0].iteration_results[0]
                        .failures
                        .insert(ResultKey::primary("latency"), vec!["timeout".into()]);
                }
                6 => wrong[1].source.as_mut().unwrap().commit_sha = Some("other".into()),
                _ => {
                    let duplicate = wrong[0].results[0].clone();
                    wrong[1].results.push(duplicate);
                }
            }
            assert!(
                BenchmarkComparison::from_pairs(&base, &wrong).is_err(),
                "accepted mutation {mutation}"
            );
        }
        assert!(BenchmarkComparison::from_pairs(&base[..2], &head[..2]).is_err());
    }

    #[test]
    fn paired_comparison_reports_missing_metrics_without_an_interval() {
        let (base, mut head) = pairs();
        head[1].results[0].results[0].iteration_results[1]
            .duration_samples
            .clear();
        let report = BenchmarkComparison::from_pairs(&base, &head).unwrap();
        assert_eq!(report.results[0].status, "incomplete-pairs");
        assert!(report.results[0].confidence_interval.is_none());
        for run in &mut head {
            run.results.clear();
        }
        assert!(
            BenchmarkComparison::from_pairs(&base, &head)
                .unwrap()
                .results[0]
                .head_observations
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn paired_comparison_qualifies_numeric_overflow_without_invalid_json() {
        let (mut base, mut head) = pairs();
        for (runs, value) in [(&mut base, 1e-300), (&mut head, 1e100)] {
            for run in runs {
                for iteration in &mut run.results[0].results[0].iteration_results {
                    iteration
                        .duration_samples
                        .insert(ResultKey::primary("latency"), vec![value; 2]);
                }
            }
        }
        let report = BenchmarkComparison::from_pairs(&base, &head).unwrap();
        assert_eq!(report.results[0].status, "unrepresentable-relative-change");
        assert!(report.results[0].confidence_interval.is_none());
        serde_json::from_str::<serde_json::Value>(&serde_json::to_string(&report).unwrap())
            .unwrap();
    }

    #[test]
    fn density_iteration_merge_retains_failed_positions_and_rejects_mixed_cells() {
        let mut inputs: Vec<_> = (0..3)
            .map(|index| run("base", index, (index + 1) as f64, 5.0))
            .collect();
        let failure = BenchmarkRecorder::new();
        failure.failure(&ResultKey::primary("latency"), "timeout");
        inputs[1].results[0].results[0].add(failure);
        inputs[1].results[0].results[0].iteration_results[1].skipped = Some("setup".into());
        let merged = BenchmarkSuiteResult::merge_iterations(inputs.clone()).unwrap();
        let result = &merged.results[0].results[0];
        assert_eq!(merged.results[0].iterations, 9);
        assert_eq!(
            result.iteration_results[4].skipped.as_deref(),
            Some("setup")
        );
        assert_eq!(result.failure_count(), 1);
        assert_eq!(
            result.duration_results[&ResultKey::primary("latency")]
                .all
                .len(),
            309
        );
        assert_eq!(
            result.count_results[&ResultKey::primary("calls")].all,
            vec![101, 1, 1, 101, 1, 1, 101, 1, 1]
        );
        for mutation in 0..4 {
            let mut wrong = inputs.clone();
            match mutation {
                0 => wrong[1].results[0].series_key = "other".into(),
                1 => wrong[1].harness_version += 1,
                2 => wrong[1].results[0].results[0].run_config.size += 1,
                _ => wrong[1].source = None,
            }
            assert!(BenchmarkSuiteResult::merge_iterations(wrong).is_err());
        }
    }

    #[test]
    fn density_merge_accepts_representable_average_of_large_counts() {
        let key = ResultKey::primary("large-count");
        let mut inputs = Vec::new();
        for index in 0..3 {
            let mut input = run("base", index, 1.0, 1.0);
            let config = input.results[0].results[0].run_config.clone();
            let mut result = BenchmarkRunResult::new(config);
            let recorder = BenchmarkRecorder::new();
            recorder.count(&key, u64::MAX / 2 + 1);
            result.add_iteration(0, recorder);
            input.results[0].iterations = 1;
            input.results[0].results = vec![result];
            inputs.push(input);
        }
        let merged = BenchmarkSuiteResult::merge_iterations(inputs).unwrap();
        let counts = &merged.results[0].results[0].count_results[&key];
        assert_eq!(counts.avg, u64::MAX / 2 + 1);
        assert_eq!(counts.all, vec![u64::MAX / 2 + 1; 3]);
    }

    #[test]
    fn density_merge_accepts_representable_average_of_large_durations() {
        let key = ResultKey::primary("large-duration");
        let observation = Duration::MAX;
        let mut inputs = Vec::new();
        for index in 0..2 {
            let mut input = run("base", index, 1.0, 1.0);
            let config = input.results[0].results[0].run_config.clone();
            let mut result = BenchmarkRunResult::new(config);
            let recorder = BenchmarkRecorder::new();
            recorder.duration(&key, observation);
            result.add_iteration(0, recorder);
            // Each input is independently valid and its mean is representable.
            assert_eq!(result.duration_results[&key].avg, observation);
            input.results[0].iterations = 1;
            input.results[0].results = vec![result];
            inputs.push(input);
        }
        let merged = BenchmarkSuiteResult::merge_iterations(inputs).unwrap();
        let durations = &merged.results[0].results[0].duration_results[&key];
        assert_eq!(durations.avg, observation);
        assert_eq!(durations.median, observation);
        assert_eq!(durations.p99, observation);
        assert_eq!(durations.all, vec![observation; 2]);
    }
}
