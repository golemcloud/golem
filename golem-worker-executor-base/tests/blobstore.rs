use crate::common;
use assert2::check;
use std::path::Path;

#[tokio::test]
async fn blobstore_exists_return_true_if_the_container_was_created() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/blob-store-service.wasm"));
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/create-container",
            vec![common::val_string(&format!(
                "{template_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/container-exists",
            vec![common::val_string(&format!(
                "{template_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_bool(true)]);
}

#[tokio::test]
async fn blobstore_exists_return_false_if_the_container_was_not_created() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/blob-store-service.wasm"));
    let worker_name = "blob-store-service-1";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/container-exists",
            vec![common::val_string(&format!(
                "{template_id}-{worker_name}-container"
            ))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_bool(false)]);
}
