use crate::common::{start, TestContext};
use assert2::check;
use golem_test_framework::dsl::TestDsl;
use golem_wasm_rpc::Value;

#[tokio::test]
#[tracing::instrument]
async fn blobstore_exists_return_true_if_the_container_was_created() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("blob-store-service").await;
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-container",
            vec![Value::String(format!(
                "{template_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/container-exists",
            vec![Value::String(format!(
                "{template_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::Bool(true)]);
}

#[tokio::test]
#[tracing::instrument]
async fn blobstore_exists_return_false_if_the_container_was_not_created() {
    let context = TestContext::new();
    let executor = start(&context).await.unwrap();

    let template_id = executor.store_template("blob-store-service").await;
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/container-exists",
            vec![Value::String(format!(
                "{template_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![Value::Bool(false)]);
}
