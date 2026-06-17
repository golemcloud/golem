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

//! Minimal worker-executor `/metrics` scraper for density cross-axis snapshots.
//!
//! Parses the subset of Prometheus text-format metrics density needs to
//! classify a ceiling's breaking point (worker-memory pool, active-workers
//! state breakdown, scheduler queue depth — golemcloud/golem#3517). Missing
//! metrics parse as `None` rather than failing: the #3517 metrics PR may not be
//! deployed on every target, and a scrape failure must never lose a ceiling
//! (ceilings are detected from driver-local latency / connection state, never
//! from these metrics).
//!
//! The scrape target is a per-cell kubectl port-forward to the freshly-
//! restarted executor pod, so the endpoint URL is `localhost`-stable for the
//! cell's duration.

use crate::benchmarks::density::ceiling::CrossAxisSnapshot;
use std::time::Duration;
use tracing::{debug, warn};

/// Scrapes `metrics_url` once and parses it into a [`CrossAxisSnapshot`].
///
/// Returns an empty snapshot (all fields `None`) on any transport or parse
/// failure, logging a warning — a failed scrape must not abort the cell.
pub async fn scrape_snapshot(client: &reqwest::Client, metrics_url: &str) -> CrossAxisSnapshot {
    match scrape_text(client, metrics_url).await {
        Ok(text) => parse_snapshot(&text),
        Err(e) => {
            warn!("density: executor /metrics scrape failed ({metrics_url}): {e:?}");
            CrossAxisSnapshot::default()
        }
    }
}

async fn scrape_text(client: &reqwest::Client, metrics_url: &str) -> anyhow::Result<String> {
    let resp = client
        .get(metrics_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("status {}", resp.status());
    }
    Ok(resp.text().await?)
}

/// Parses a Prometheus text-format exposition into a [`CrossAxisSnapshot`].
pub fn parse_snapshot(text: &str) -> CrossAxisSnapshot {
    let mut snapshot = CrossAxisSnapshot::default();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((metric, value)) = parse_sample_line(line) else {
            continue;
        };

        match metric.name {
            "golem_worker_memory_pool_total_bytes" => {
                snapshot.worker_memory_pool_total_bytes = Some(value as u64);
            }
            "golem_worker_memory_pool_used_bytes" => {
                snapshot.worker_memory_pool_used_bytes = Some(value as u64);
            }
            "golem_active_workers" => match metric.label("state") {
                Some("running") => snapshot.active_workers_running = Some(value as u64),
                Some("unloaded") => snapshot.active_workers_unloaded = Some(value as u64),
                Some("waiting_for_permit") => {
                    snapshot.active_workers_waiting_for_permit = Some(value as u64)
                }
                Some("stopping") => snapshot.active_workers_stopping = Some(value as u64),
                _ => {}
            },
            "scheduler_queue_depth" => {
                snapshot.scheduler_queue_depth = Some(value as u64);
            }
            _ => {}
        }
    }

    debug!("density: parsed executor snapshot: {snapshot:?}");
    snapshot
}

/// A parsed metric line's name and (optional) labels.
struct ParsedMetric<'a> {
    name: &'a str,
    labels: &'a str,
}

impl ParsedMetric<'_> {
    /// Returns the value of label `key`, if present. Cheap substring scan —
    /// the lines we care about have at most a couple of labels.
    fn label(&self, key: &str) -> Option<&str> {
        for pair in self.labels.split(',') {
            let pair = pair.trim();
            if let Some(rest) = pair.strip_prefix(key) {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let rest = rest.trim().trim_matches('"');
                    return Some(rest);
                }
            }
        }
        None
    }
}

/// Parses one `name{labels} value [timestamp]` sample line into its metric and
/// numeric value. Returns `None` for lines that do not match.
fn parse_sample_line(line: &str) -> Option<(ParsedMetric<'_>, f64)> {
    // Split name+labels from the value (and optional timestamp).
    let (name_labels, rest) = if let Some(brace_end) = line.find('}') {
        // name{labels} value
        let after = &line[brace_end + 1..];
        (&line[..=brace_end], after.trim())
    } else {
        // name value
        let space = line.find(char::is_whitespace)?;
        (&line[..space], line[space..].trim())
    };

    let value_str = rest.split_whitespace().next()?;
    let value: f64 = value_str.parse().ok()?;

    let (name, labels) = if let Some(brace) = name_labels.find('{') {
        let name = &name_labels[..brace];
        let labels = name_labels[brace + 1..].trim_end_matches('}');
        (name, labels)
    } else {
        (name_labels, "")
    };

    Some((ParsedMetric { name, labels }, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn parses_memory_pool_gauges() {
        let text = "\
# HELP golem_worker_memory_pool_total_bytes total
# TYPE golem_worker_memory_pool_total_bytes gauge
golem_worker_memory_pool_total_bytes{executor_id=\"exec-0\"} 11166914560
golem_worker_memory_pool_used_bytes{executor_id=\"exec-0\"} 5368709120
";
        let s = parse_snapshot(text);
        assert_eq!(s.worker_memory_pool_total_bytes, Some(11166914560));
        assert_eq!(s.worker_memory_pool_used_bytes, Some(5368709120));
    }

    #[test]
    fn parses_active_workers_by_state() {
        let text = "\
golem_active_workers{executor_id=\"exec-0\",state=\"running\"} 120
golem_active_workers{executor_id=\"exec-0\",state=\"unloaded\"} 30
golem_active_workers{executor_id=\"exec-0\",state=\"waiting_for_permit\"} 5
golem_active_workers{executor_id=\"exec-0\",state=\"stopping\"} 2
";
        let s = parse_snapshot(text);
        assert_eq!(s.active_workers_running, Some(120));
        assert_eq!(s.active_workers_unloaded, Some(30));
        assert_eq!(s.active_workers_waiting_for_permit, Some(5));
        assert_eq!(s.active_workers_stopping, Some(2));
    }

    #[test]
    fn parses_scheduler_queue_depth() {
        let text = "scheduler_queue_depth{executor_id=\"exec-0\"} 42\n";
        let s = parse_snapshot(text);
        assert_eq!(s.scheduler_queue_depth, Some(42));
    }

    #[test]
    fn missing_metrics_are_none() {
        // A scrape from an executor without the #3517 metrics PR deployed.
        let text = "\
# HELP some_other_metric foo
some_other_metric 1
process_cpu_seconds_total 3.14
";
        let s = parse_snapshot(text);
        assert_eq!(s, CrossAxisSnapshot::default());
    }

    #[test]
    fn ignores_floats_and_histograms() {
        // Histogram buckets share a prefix but are not gauges we read.
        let text = "\
scheduled_action_lag_seconds_bucket{le=\"0.5\"} 10
scheduler_queue_depth 7
golem_worker_memory_pool_used_bytes 123.0
";
        let s = parse_snapshot(text);
        assert_eq!(s.scheduler_queue_depth, Some(7));
        assert_eq!(s.worker_memory_pool_used_bytes, Some(123));
    }
}
