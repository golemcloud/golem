use crate::common;
use assert2::check;
use http_02::{Response, StatusCode};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tonic::transport::Body;
use warp::Filter;

#[tokio::test]
#[tracing::instrument]
async fn jump() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let host_http_port = context.host_http_port();

    let http_server = tokio::spawn(async move {
        let call_count_per_step = Arc::new(Mutex::new(HashMap::<u64, u64>::new()));
        let route = warp::path("step")
            .and(warp::path::param())
            .and(warp::get())
            .map(move |step: u64| {
                let mut steps = call_count_per_step.lock().unwrap();
                let step_count = steps.entry(step).and_modify(|e| *e += 1).or_insert(0);

                println!("step: {step} occurrence {step_count}");

                match &step_count {
                    0 => Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("true"))
                        .unwrap(),
                    _ => Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("false"))
                        .unwrap(),
                }
            });

        warp::serve(route)
            .run(
                format!("0.0.0.0:{}", host_http_port)
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await;
    });

    let template_id = executor.store_template(Path::new("../test-templates/runtime-service.wasm"));

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), context.host_http_port().to_string());

    let worker_id = executor
        .try_start_worker_versioned(&template_id, 0, "runtime-service-jump", vec![], env)
        .await
        .unwrap();

    let (rx, abort_capture) = executor.capture_output_forever(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/jump", vec![])
        .await
        .unwrap();

    drop(executor);
    http_server.abort();

    abort_capture.send(()).unwrap();
    let mut events = common::drain_connection(rx).await;
    events.retain(|e| match e {
        Some(e) => {
            !common::stdout_event_starting_with(e, "Sending")
                && !common::stdout_event_starting_with(e, "Received")
        }
        None => false,
    });

    println!("events: {:?}", events);

    check!(result == vec![common::val_u64(5)]);
    check!(
        events
            == vec![
                Some(common::stdout_event("started: 0\n")),
                Some(common::stdout_event("second: 2\n")),
                Some(common::stdout_event("second: 2\n")),
                Some(common::stdout_event("third: 3\n")),
                Some(common::stdout_event("fourth: 4\n")),
                Some(common::stdout_event("fourth: 4\n")),
                Some(common::stdout_event("fifth: 5\n")),
            ]
    );
}

#[tokio::test]
async fn set_retry_policy() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/runtime-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "set-retry-policy-1")
        .await;

    executor.log_output(&worker_id).await;

    let start = SystemTime::now();
    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/fail-with-custom-max-retries",
            vec![common::val_u64(2)],
        )
        .await;
    let elapsed = start.elapsed().unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/fail-with-custom-max-retries",
            vec![common::val_u64(1)],
        )
        .await;

    drop(executor);

    check!(elapsed < Duration::from_secs(3)); // 2 retry attempts, 1s delay
    check!(result1.is_err());
    check!(result2.is_err());
    check!(result1
        .clone()
        .err()
        .unwrap()
        .to_string()
        .starts_with("Runtime error: error while executing at wasm backtrace:"));
    check!(result2
        .err()
        .unwrap()
        .to_string()
        .starts_with("The previously invoked function failed"));
}
