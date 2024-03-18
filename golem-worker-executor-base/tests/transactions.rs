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

    let rx = executor.capture_output_with_termination(&worker_id).await;

    let result = executor
        .invoke_and_await(&worker_id, "golem:it/api/jump", vec![])
        .await
        .unwrap();

    let events = common::drain_connection(rx).await;
    drop(executor);

    println!("events: {:?}", events);

    check!(result == vec![common::val_u64(5)]);
}
