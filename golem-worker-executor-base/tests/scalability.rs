use crate::common;
use crate::common::{TestWorkerExecutor, TestWorkerExecutorClone};
use assert2::check;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use golem_common::model::TemplateId;
use std::future::Future;
use std::path::Path;
use std::time::Duration;
use tokio::spawn;
use tracing::info;

#[tokio::test]
async fn spawning_many_instances_that_sleep() {
    fn worker_name(n: i32) -> String {
        format!("sleeping-worker-{}", n)
    }

    async fn timed<F>(f: F) -> (F::Output, Duration)
    where
        F: Future + Send + 'static,
    {
        let start = tokio::time::Instant::now();
        let result = f.await;
        let duration = start.elapsed();
        (result, duration)
    }

    let mut executor = common::start().await.unwrap();
    let template_id = executor.store_template(Path::new("../test-templates/clocks.wasm"));

    let warmup_worker = executor.start_worker(&template_id, &worker_name(0)).await;

    let mut executor_clone = executor.async_clone().await;
    let warmup_result = timed(async move {
        executor_clone
            .invoke_and_await(&warmup_worker, "run", vec![])
            .await
            .unwrap()
    })
    .await;

    info!("Warmup: {:?}", warmup_result);

    const N: i32 = 100; // TODO: it gets into deadlock with 100, but works with 50 ?!
    info!("{N} instances");

    let start = tokio::time::Instant::now();
    let input: Vec<(i32, TemplateId, TestWorkerExecutorClone)> = (1..N)
        .into_iter()
        .map(|i| (i, template_id.clone(), executor.clone_info()))
        .collect();
    let fibers: Vec<_> = input
        .into_iter()
        .map(|(n, template_id, clone_info)| {
            spawn(async move {
                let mut executor_clone = TestWorkerExecutor::from_clone_info(clone_info).await;
                let worker = executor_clone
                    .start_worker(&template_id, &worker_name(n))
                    .await;
                let r = timed(async move {
                    executor_clone
                        .invoke_and_await(&worker, "run", vec![])
                        .await
                        .unwrap()
                })
                .await;

                info!("** FINISHED {n} ***");

                r
            })
        })
        .collect();

    info!("Spawned all, waiting...");
    let futures: FuturesUnordered<_> = fibers.into_iter().collect();
    let results: Vec<_> = futures.collect().await;

    let total_duration = start.elapsed();

    info!("Results: {:?}", results);
    info!("Total duration: {:?}", total_duration);

    let mut sorted = results
        .into_iter()
        .filter_map(|r| match r {
            Ok((_, duration)) => Some(duration.as_millis()),
            Err(err) => panic!("Error: {:?}", err),
        })
        .collect::<Vec<_>>();
    sorted.sort();
    let idx = (sorted.len() as f64 * 0.95) as usize;
    let p95 = sorted[idx];

    drop(executor);

    check!(p95 < 6000);
    check!(total_duration.as_secs() < 10);
}
