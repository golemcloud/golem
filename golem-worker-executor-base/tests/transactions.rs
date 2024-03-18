use crate::common;
use assert2::check;
use std::path::Path;

#[tokio::test]
async fn jump() {
    let context = common::TestContext::new();
    let mut executor = common::start(&context).await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/runtime-service.wasm"));
    let worker_id = executor
        .start_worker(&template_id, "runtime-service-jump")
        .await;

    let (rx, abort_capture) = executor.capture_output_forever(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/jump", vec![])
        .await
        .unwrap();

    drop(executor);

    abort_capture.send(()).unwrap();
    let events = common::drain_connection(rx).await;

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
