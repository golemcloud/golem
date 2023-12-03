use std::path::Path;
use std::time::Duration;

#[allow(dead_code)]
mod common;

#[tokio::test]
async fn write_stdout() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new("../test-templates/write-stdout.wasm"));
    let worker_id = executor.start_worker(&template_id, "write-stdout-1").await;
    println!("Worker started with id: {}", worker_id);

    let mut rx = executor.capture_output(&worker_id).await;

    let result = executor.invoke_and_await(&worker_id, "run", vec![]).await;
    println!("Invoked function returned with {result:?}");

    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    drop(executor);

    assert_eq!(
        events,
        vec![common::stdout_event("Sample text written to the output\n")]
    );
}
