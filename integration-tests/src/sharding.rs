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
use futures::StreamExt;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;
use tokio_stream::wrappers::IntervalStream;

/// Polls startup and both earlier executor counts together from the first poll.
/// Retains the count result until startup completes, even after the observation expires.
/// Accepts only a complete matching batch processed strictly before the launch deadline.
pub async fn observe_executor_count<S, F, Fut>(
    expected: usize,
    within: Duration,
    startup: S,
    mut scrape: F,
) -> (S::Output, anyhow::Result<()>)
where
    S: Future,
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Box<[String]>>>,
{
    let deadline = tokio::time::Instant::now() + within;
    let count = async {
        let last_scrape_error = Mutex::new(None);
        let observed = IntervalStream::new(tokio::time::interval(Duration::from_millis(100)))
            .then(|_| scrape())
            .filter_map(|reported| {
                let result = match reported {
                    Err(error) if error.downcast_ref::<reqwest::Error>().is_some() => {
                        *last_scrape_error.lock().unwrap() = Some(error);
                        None
                    }
                    Err(error) => Some(Err(error)),
                    Ok(metrics) => batch_matches(expected, &metrics).transpose(),
                };
                std::future::ready(result)
            });
        tokio::pin!(observed);
        let result = tokio::time::timeout_at(deadline, observed.next())
            .await
            .with_context(|| {
                format!(
                    "count {expected} was not observed by the deadline; latest scrape error: {:?}",
                    last_scrape_error.lock().unwrap()
                )
            })?
            .context("the observation stream ended")?;
        ensure!(
            tokio::time::Instant::now() < deadline,
            "count {expected} was not observed by the deadline; latest scrape error: {:?}",
            last_scrape_error.lock().unwrap()
        );
        result
    };
    tokio::join!(startup, count)
}

fn batch_matches(expected: usize, metrics: &[String]) -> anyhow::Result<Option<()>> {
    ensure!(
        metrics.len() == 2,
        "expected metrics from both earlier executors"
    );
    let ready = metrics.iter().try_fold(true, |ready, metrics| {
        let count = sample(metrics, "executor_count")?;
        anyhow::Ok(ready && count == expected as u64)
    })?;
    Ok(ready.then_some(()))
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
    use std::task::Poll;
    use std::time::Duration;
    use test_r::test;
    use tokio::sync::{mpsc, oneshot};

    fn paused<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(future)
    }

    fn metrics(counts: [usize; 2]) -> Box<[String]> {
        Box::new(counts.map(|count| format!("executor_count {count}\n")))
    }

    #[test]
    fn expired_count_drops_the_scrape_but_retains_pending_startup() {
        paused(async {
            let (readiness_tx, readiness_rx) = oneshot::channel();
            let (batch_tx, batch_rx) = oneshot::channel::<Box<[String]>>();
            let mut first_batch = Some(batch_rx);
            let startup = async move {
                readiness_rx.await.unwrap();
                Err::<(), _>("startup sentinel")
            };
            let observed = observe_executor_count(3, Duration::from_secs(60), startup, || {
                let batch = first_batch.take().expect("the scrape must not restart");
                async move { batch.await.context("the batch must arrive") }
            });
            tokio::pin!(observed);
            assert!(futures::poll!(observed.as_mut()).is_pending());
            tokio::time::advance(Duration::ZERO).await;
            assert!(futures::poll!(observed.as_mut()).is_pending());
            assert!(!batch_tx.is_closed());
            tokio::time::advance(Duration::from_secs(61)).await;
            assert!(futures::poll!(observed.as_mut()).is_pending());
            assert!(batch_tx.is_closed(), "expiry must drop the stalled scrape");
            tokio::time::advance(Duration::from_secs(14)).await;
            readiness_tx.send(()).expect("startup must remain alive");
            let Poll::Ready((startup_output, result)) = futures::poll!(observed.as_mut()) else {
                panic!("expiry must be retained after readiness");
            };
            assert_eq!(startup_output, Err("startup sentinel"));
            assert!(result.unwrap_err().to_string().contains("deadline"));
        });
    }

    #[test]
    fn readiness_first_keeps_the_original_launch_deadline() {
        [(5, 20, true), (50, 70, false)]
            .into_iter()
            .for_each(|(ready_at, batch_at, expected)| {
                paused(async move {
                    let (readiness_tx, readiness_rx) = oneshot::channel();
                    let (batch_tx, batch_rx) = oneshot::channel();
                    let mut first_batch = Some(batch_rx);
                    let startup = async move {
                        readiness_rx.await.unwrap();
                        91
                    };
                    let observed =
                        observe_executor_count(3, Duration::from_secs(60), startup, || {
                            let batch = first_batch.take().unwrap();
                            async move { batch.await.context("the batch must arrive") }
                        });
                    tokio::pin!(observed);
                    assert!(futures::poll!(observed.as_mut()).is_pending());
                    tokio::time::advance(Duration::ZERO).await;
                    assert!(futures::poll!(observed.as_mut()).is_pending());
                    tokio::time::advance(Duration::from_secs(ready_at)).await;
                    readiness_tx.send(()).unwrap();
                    assert!(futures::poll!(observed.as_mut()).is_pending());
                    tokio::time::advance(Duration::from_secs(batch_at - ready_at)).await;
                    batch_tx.send(metrics([3, 3])).unwrap();
                    let Poll::Ready((startup_output, result)) = futures::poll!(observed.as_mut())
                    else {
                        panic!("the completed batch must resolve the observation");
                    };
                    assert_eq!(startup_output, 91);
                    assert_eq!(result.is_ok(), expected);
                })
            });
    }

    #[test]
    fn zero_window_still_polls_and_retains_startup() {
        paused(async {
            let (readiness_tx, readiness_rx) = oneshot::channel();
            let (entered_tx, mut entered_rx) = oneshot::channel();
            let startup = async move {
                entered_tx.send(()).unwrap();
                readiness_rx.await.unwrap();
                87
            };
            let observed =
                observe_executor_count(3, Duration::ZERO, startup, || ready(Ok(metrics([3, 3]))));
            tokio::pin!(observed);
            assert!(futures::poll!(observed.as_mut()).is_pending());
            tokio::time::advance(Duration::ZERO).await;
            assert!(futures::poll!(observed.as_mut()).is_pending());
            entered_rx
                .try_recv()
                .expect("startup must be polled even with an expired observation");
            readiness_tx.send(()).unwrap();
            let (startup_output, result) = observed.await;
            assert_eq!(startup_output, 87);
            assert!(result.is_err());
        });
    }

    #[test]
    fn launch_is_the_first_poll_not_future_construction() {
        paused(async {
            let observed = observe_executor_count(3, Duration::from_secs(60), ready(37), || {
                ready(Ok(metrics([3, 3])))
            });
            tokio::time::advance(Duration::from_secs(120)).await;
            let (startup_output, result) = observed.await;
            assert_eq!(startup_output, 37);
            result.unwrap();
        });
    }

    #[test]
    fn errors_are_terminal_without_cancelling_pending_startup() {
        [
            Err(anyhow::anyhow!("executor has no metrics endpoint")),
            Ok(Box::new([]) as Box<[String]>),
            Ok(Box::new(["executor_count 3\n".to_owned()]) as Box<[String]>),
            Ok(
                Box::new([3, 3, 3].map(|count| format!("executor_count {count}\n")))
                    as Box<[String]>,
            ),
            Ok(Box::new([
                "executor_count 2\n".to_owned(),
                "executor_count invalid\n".to_owned(),
            ]) as Box<[String]>),
        ]
        .into_iter()
        .for_each(|batch| {
            paused(async move {
                let (readiness_tx, readiness_rx) = oneshot::channel();
                let startup = async move {
                    readiness_rx.await.unwrap();
                    43
                };
                let mut batch = Some(batch);
                let observed = observe_executor_count(3, Duration::from_secs(60), startup, || {
                    ready(batch.take().expect("ordinary errors must not retry"))
                });
                tokio::pin!(observed);
                assert!(futures::poll!(observed.as_mut()).is_pending());
                tokio::time::advance(Duration::ZERO).await;
                assert!(futures::poll!(observed.as_mut()).is_pending());
                readiness_tx
                    .send(())
                    .expect("ordinary errors must retain startup");
                let Poll::Ready((startup_output, result)) = futures::poll!(observed.as_mut())
                else {
                    panic!("an ordinary observation error must not retry");
                };
                assert_eq!(startup_output, 43);
                assert!(result.is_err());
            })
        });
    }

    #[test]
    fn executor_counts_do_not_accumulate_across_batches() {
        paused(async {
            let mut batches = [[3, 2], [2, 3]].into_iter();
            let (_, result) = observe_executor_count(3, Duration::from_secs(1), ready(()), || {
                ready(Ok(metrics(batches.next().unwrap_or([2, 3]))))
            })
            .await;
            assert!(result.unwrap_err().to_string().contains("deadline"));
        });
    }

    #[test]
    fn count_mismatches_retry_until_one_complete_batch_matches() {
        paused(async {
            let started_at = tokio::time::Instant::now();
            let mut batches = [[3, 2], [2, 3], [3, 3]].into_iter();
            let (_, result) = observe_executor_count(3, Duration::from_secs(1), ready(()), || {
                ready(Ok(metrics(
                    batches.next().expect("the third batch matches"),
                )))
            })
            .await;
            result.unwrap();
            assert_eq!(started_at.elapsed(), Duration::from_millis(200));
        });
    }

    #[test]
    fn contextual_transport_errors_retry_and_remain_in_expiry_diagnostics() {
        [false, true].into_iter().for_each(|recover| {
            paused(async move {
                let mut first = true;
                let (_, result) =
                    observe_executor_count(3, Duration::from_secs(1), ready(()), || {
                        let batch = if first || !recover {
                            let error = reqwest::Response::from(
                                http::Response::builder().status(503).body("").unwrap(),
                            )
                            .error_for_status()
                            .unwrap_err();
                            Err(anyhow::Error::new(error).context("temporary metrics request"))
                        } else {
                            Ok(metrics([3, 3]))
                        };
                        first = false;
                        ready(batch)
                    })
                    .await;
                if recover {
                    result.unwrap();
                } else {
                    let error = result.unwrap_err().to_string();
                    assert!(error.contains("deadline"));
                    assert!(error.contains("temporary metrics request"));
                }
            })
        });
    }

    #[test]
    fn a_ready_batch_at_or_after_the_deadline_is_rejected() {
        [60, 61].into_iter().for_each(|seconds| {
            paused(async move {
                let (batch_tx, batch_rx) = oneshot::channel();
                let mut first_batch = Some(batch_rx);
                let observed =
                    observe_executor_count(3, Duration::from_secs(60), ready(73), || {
                        let batch = first_batch.take().expect("only the gated batch is needed");
                        async move { batch.await.context("the gated batch must arrive") }
                    });
                tokio::pin!(observed);
                assert!(futures::poll!(observed.as_mut()).is_pending());
                tokio::time::advance(Duration::ZERO).await;
                assert!(futures::poll!(observed.as_mut()).is_pending());
                tokio::time::advance(Duration::from_secs(seconds)).await;
                batch_tx.send(metrics([3, 3])).unwrap();
                let Poll::Ready((startup_output, result)) = futures::poll!(observed.as_mut())
                else {
                    panic!("the expired count must complete");
                };
                assert_eq!(startup_output, 73);
                assert!(
                    result.is_err(),
                    "a batch accepted at {seconds}s is too late"
                );
            })
        });
    }

    #[test]
    fn in_window_count_survives_late_readiness() {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(async {
                let (readiness_tx, readiness_rx) = oneshot::channel();
                let (startup_entered_tx, mut startup_entered_rx) = oneshot::channel();
                let (scrape_entered_tx, mut scrape_entered_rx) = mpsc::unbounded_channel();
                let (batch_tx, batch_rx) = oneshot::channel::<Box<[String]>>();
                let mut first_batch = Some(batch_rx);
                let startup = async move {
                    startup_entered_tx.send(()).unwrap();
                    readiness_rx.await.unwrap();
                    73
                };
                let observed = observe_executor_count(3, Duration::from_secs(60), startup, || {
                    scrape_entered_tx.send(()).unwrap();
                    let batch = first_batch.take();
                    async move {
                        match batch {
                            Some(batch) => batch.await.context("the timely batch must arrive"),
                            None => Ok(Box::new(
                                [2, 2].map(|count| format!("executor_count {count}\n")),
                            ) as Box<[String]>),
                        }
                    }
                });
                tokio::pin!(observed);

                assert!(futures::poll!(observed.as_mut()).is_pending());
                tokio::time::advance(Duration::ZERO).await;
                assert!(futures::poll!(observed.as_mut()).is_pending());
                startup_entered_rx
                    .try_recv()
                    .expect("startup must enter while readiness is closed");
                scrape_entered_rx
                    .try_recv()
                    .expect("scraping must start while readiness is closed");

                tokio::time::advance(Duration::from_secs(20)).await;
                batch_tx
                    .send(Box::new(
                        [3, 3].map(|count| format!("executor_count {count}\n")),
                    ))
                    .unwrap();
                assert!(futures::poll!(observed.as_mut()).is_pending());

                tokio::time::advance(Duration::from_secs(55)).await;
                assert!(futures::poll!(observed.as_mut()).is_pending());
                readiness_tx
                    .send(())
                    .expect("count observation must retain the pending startup");
                let Poll::Ready((startup_output, count_result)) = futures::poll!(observed.as_mut())
                else {
                    panic!("the accepted count must not require another scrape after readiness");
                };
                assert_eq!(startup_output, 73);
                count_result.expect("the in-window count must survive readiness after the lease");
            });
    }

    #[test]
    fn both_executor_counts_are_observed_without_a_delivery_claim() {
        [([3, 3], true), ([2, 3], false), ([3, 2], false)]
            .into_iter()
            .for_each(|(counts, expected)| {
                paused(async {
                    let (_, observed) =
                        observe_executor_count(3, Duration::from_secs(1), ready(()), || {
                            ready(Ok(Box::new(
                                counts.map(|count| format!("executor_count {count}\n")),
                            ) as Box<[String]>))
                        })
                        .await;
                    assert_eq!(observed.is_ok(), expected);
                });
            });
    }

    #[test]
    fn missing_or_invalid_counts_are_errors_on_either_executor() {
        ["", "executor_count invalid\n"]
            .into_iter()
            .flat_map(|metrics| [0, 1].map(move |executor| (metrics, executor)))
            .flat_map(|(metrics, executor)| [2, 3].map(move |other| (metrics, executor, other)))
            .for_each(|(metrics, executor, other)| {
                paused(async {
                    let (_, observed) =
                        observe_executor_count(3, Duration::from_secs(1), ready(()), || {
                            let mut batch =
                                [other, other].map(|count| format!("executor_count {count}\n"));
                            batch[executor] = metrics.to_owned();
                            ready(Ok(Box::new(batch) as Box<[String]>))
                        })
                        .await;
                    let error = observed.unwrap_err().to_string();
                    assert!(error.contains("metric executor_count"), "{error}");
                });
            });
    }
}
