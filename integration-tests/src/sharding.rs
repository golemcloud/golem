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

use anyhow::{Context, ensure};
use std::future::Future;

/// Observes the count on both earlier executors, then reads their delivery attempts again.
/// Returns register, assign and revoke totals separately for each executor, in scrape order.
/// The follow-up batch starts after the entire count batch completes. Processes must not restart.
pub async fn observe_executor_count<F, Fut>(
    expected: usize,
    mut scrape: F,
) -> anyhow::Result<Option<Box<[[u64; 3]]>>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Box<[String]>>>,
{
    let metrics = scrape().await?;
    ensure!(
        metrics.len() == 2,
        "expected metrics from both earlier executors"
    );
    let ready = metrics.iter().try_fold(true, |ready, metrics| {
        let count = sample(metrics, "executor_count")?;
        let attempts = delivery_attempts(metrics)?;
        anyhow::Ok(ready && count == expected as u64 && attempts[1] > 0)
    })?;
    if !ready {
        return Ok(None);
    }

    // A response can contain an old counter and a new gauge. Read attempts after observing count.
    let metrics = scrape().await?;
    ensure!(
        metrics.len() == 2,
        "expected metrics from both earlier executors"
    );
    let attempts = metrics
        .iter()
        .map(|metrics| delivery_attempts(metrics))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_boxed_slice();
    Ok(Some(attempts))
}

fn delivery_attempts(metrics: &str) -> anyhow::Result<[u64; 3]> {
    let [register, assign, revoke] = ["register", "assign", "revoke"].map(|delivery| {
        sample(
            metrics,
            &format!("shard_delivery_attempts_total{{delivery=\"{delivery}\"}}"),
        )
    });
    Ok([register?, assign?, revoke?])
}

fn sample(metrics: &str, name: &str) -> anyhow::Result<u64> {
    metrics
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .with_context(|| format!("missing required metric {name}"))?
        .parse()
        .with_context(|| format!("invalid metric {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    #[test]
    async fn a_pending_assignment_that_published_three_is_not_renewal_only() {
        let baseline = "executor_count 2\n\
            shard_delivery_attempts_total{delivery=\"register\"} 1\n\
            shard_delivery_attempts_total{delivery=\"assign\"} 1\n\
            shard_delivery_attempts_total{delivery=\"revoke\"} 0\n\
            api_success_seconds_count{api=\"assign_shards\",api_type=\"grpc\"} 1\n";
        let pending = "executor_count 3\n\
            shard_delivery_attempts_total{delivery=\"register\"} 1\n\
            shard_delivery_attempts_total{delivery=\"assign\"} 2\n\
            shard_delivery_attempts_total{delivery=\"revoke\"} 0\n\
            api_success_seconds_count{api=\"assign_shards\",api_type=\"grpc\"} 1\n";
        let before = observe_executor_count(2, || {
            ready(Ok(
                Box::new([baseline.to_owned(), baseline.to_owned()]) as Box<[String]>
            ))
        })
        .await
        .unwrap()
        .unwrap();
        let after = observe_executor_count(3, || {
            ready(Ok(
                Box::new([pending.to_owned(), pending.to_owned()]) as Box<[String]>
            ))
        })
        .await
        .unwrap()
        .unwrap();
        assert_ne!(
            after, before,
            "a pending push must invalidate renewal-only evidence"
        );
    }

    fn metrics(count: usize, attempts: [u64; 3], successes: u64, failures: u64) -> String {
        let [register, assign, revoke] = attempts;
        format!(
            "executor_count {count}\n\
             shard_delivery_attempts_total{{delivery=\"register\"}} {register}\n\
             shard_delivery_attempts_total{{delivery=\"assign\"}} {assign}\n\
             shard_delivery_attempts_total{{delivery=\"revoke\"}} {revoke}\n\
             api_success_seconds_count{{api=\"assign_shards\",api_type=\"grpc\"}} {successes}\n\
             api_failure_seconds_count{{api=\"assign_shards\",api_type=\"grpc\"}} {failures}\n"
        )
    }

    async fn observe<const N: usize>(
        count: usize,
        script: [[String; 2]; N],
    ) -> anyhow::Result<Option<Box<[[u64; 3]]>>> {
        let mut script = script.into_iter();
        let observed = observe_executor_count(count, || {
            ready(
                script
                    .next()
                    .map(|batch| Box::new(batch) as Box<[String]>)
                    .context("unexpected scrape"),
            )
        })
        .await;
        if observed.is_ok() {
            assert_eq!(script.len(), 0, "every scripted batch must be observed");
        }
        observed
    }

    #[test]
    async fn failed_and_reregistered_deliveries_are_not_renewal_only() {
        let baseline = [[1, 2, 0], [1, 3, 1]];
        futures::future::join_all(
            [
                ([1, 3, 0], [1, 3, 1], 1),
                ([2, 2, 0], [1, 3, 1], 0),
                ([1, 2, 1], [1, 3, 1], 0),
                ([1, 2, 0], [2, 3, 1], 0),
                ([1, 2, 0], [1, 4, 1], 0),
                ([1, 2, 0], [1, 3, 2], 0),
                ([1, 3, 0], [1, 2, 1], 0),
            ]
            .into_iter()
            .map(|(first, second, failures)| async move {
                let observed = observe(
                    3,
                    std::array::from_fn::<_, 2, _>(|_| {
                        [metrics(3, first, 1, failures), metrics(3, second, 1, 0)]
                    }),
                )
                .await
                .unwrap()
                .unwrap();
                assert_ne!(
                    &*observed, &baseline,
                    "each executor's attempts must match separately"
                );
            }),
        )
        .await;
    }

    #[test]
    async fn the_follow_up_detects_a_torn_count_scrape() {
        let observed = observe(
            3,
            [
                [metrics(3, [1, 1, 0], 1, 0), metrics(3, [1, 1, 0], 1, 0)],
                [metrics(3, [1, 2, 0], 1, 0), metrics(3, [1, 1, 0], 1, 0)],
            ],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&*observed, &[[1, 2, 0], [1, 1, 0]]);
    }

    #[test]
    async fn renewal_only_counts_ignore_late_startup_completions() {
        let before = observe(
            2,
            [
                [metrics(2, [1, 2, 0], 1, 0), metrics(2, [1, 1, 1], 0, 0)],
                [metrics(2, [1, 2, 0], 1, 0), metrics(2, [1, 1, 1], 0, 0)],
            ],
        )
        .await
        .unwrap()
        .unwrap();
        let after = observe(
            3,
            [
                [metrics(3, [1, 2, 0], 2, 0), metrics(3, [1, 1, 1], 1, 0)],
                [metrics(3, [1, 2, 0], 2, 0), metrics(3, [1, 1, 1], 1, 0)],
            ],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            after, before,
            "completion totals do not identify delivery entry"
        );
    }

    #[test]
    async fn missing_required_series_is_an_error_in_either_batch() {
        futures::future::join_all(
            [
                "executor_count",
                "shard_delivery_attempts_total{delivery=\"register\"}",
                "shard_delivery_attempts_total{delivery=\"assign\"}",
                "shard_delivery_attempts_total{delivery=\"revoke\"}",
            ]
            .into_iter()
            .flat_map(|missing| [0, 1].map(move |batch| (missing, batch)))
            .filter(|(missing, batch)| *batch == 0 || *missing != "executor_count")
            .map(|(missing, batch)| async move {
                let mut script = std::array::from_fn::<_, 2, _>(|_| {
                    [metrics(3, [1, 1, 0], 1, 0), metrics(3, [1, 1, 0], 1, 0)]
                });
                script[batch][1] = script[batch][1]
                    .lines()
                    .filter(|line| !line.starts_with(missing))
                    .collect::<Vec<_>>()
                    .join("\n");
                let error = observe(3, script).await.unwrap_err();
                assert!(
                    error.to_string().contains("missing required metric"),
                    "{error}"
                );
            }),
        )
        .await;
    }

    #[test]
    async fn incomplete_counts_or_startup_do_not_produce_a_snapshot() {
        futures::future::join_all([(2, [1, 1, 0]), (3, [1, 0, 0])].into_iter().map(
            |(count, attempts)| async move {
                assert!(
                    observe(
                        3,
                        [[metrics(3, [1, 1, 0], 1, 0), metrics(count, attempts, 1, 0)]]
                    )
                    .await
                    .unwrap()
                    .is_none()
                );
            },
        ))
        .await;
    }

    #[test]
    async fn the_whole_count_batch_precedes_the_follow_up() {
        use std::sync::Arc;
        let gate = Arc::new(tokio::sync::Notify::new());
        let calls = AtomicUsize::new(0);
        let observation = observe_executor_count(3, || {
            let first_batch = calls.fetch_add(1, Ordering::SeqCst) == 0;
            let gate = gate.clone();
            async move {
                let first = metrics(3, [1, 1, 0], 1, 0);
                if first_batch {
                    gate.notified().await;
                }
                Ok(Box::new([first, metrics(3, [1, 1, 0], 1, 0)]) as Box<[String]>)
            }
        });
        tokio::pin!(observation);
        assert!(futures::poll!(&mut observation).is_pending());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the second executor's count is still pending"
        );
        gate.notify_one();
        assert_eq!(
            &*observation.await.unwrap().unwrap(),
            &[[1, 1, 0], [1, 1, 0]]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    async fn the_deadline_includes_the_follow_up_batch() {
        let calls = AtomicUsize::new(0);
        let observation = observe_executor_count(3, || {
            let first_batch = calls.fetch_add(1, Ordering::SeqCst) == 0;
            async move {
                if !first_batch {
                    std::future::pending::<()>().await;
                }
                Ok(
                    Box::new([metrics(3, [1, 1, 0], 1, 0), metrics(3, [1, 1, 0], 1, 0)])
                        as Box<[String]>,
                )
            }
        });
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, observation)
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
