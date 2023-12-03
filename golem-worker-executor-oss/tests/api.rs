use std::path::Path;

#[allow(dead_code)]
mod common;

#[tokio::test]
async fn shopping_cart_example() {
    let mut executor = common::start().await.unwrap();

    let template_id = executor.store_template(Path::new(
        "../test-templates/shopping-cart.wasm",
    ));
    let worker_id = executor.start_worker(&template_id, "shopping-cart-1").await;
    println!("Worker started with id: {}", worker_id);

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/initialize-cart",
            vec![common::val_string("test-user-1")],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1001"),
                common::val_string("Golem Cloud Subscription 1y"),
                common::val_float32(999999.0),
                common::val_u32(1),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/add-item",
            vec![common::val_record(vec![
                common::val_string("G1002"),
                common::val_string("Mud Golem"),
                common::val_float32(11.0),
                common::val_u32(10),
            ])],
        )
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/update-item-quantity",
            vec![common::val_string("G1002"), common::val_u32(20)],
        )
        .await;

    let contents = executor
        .invoke_and_await(&worker_id, "golem:it/api/get-cart-contents", vec![])
        .await;

    let _ = executor
        .invoke_and_await(&worker_id, "golem:it/api/checkout", vec![])
        .await;

    drop(executor);

    assert_eq!(
        contents,
        vec![common::val_list(vec![
            common::val_record(vec![
                common::val_string("G1000"),
                common::val_string("Golem T-Shirt M"),
                common::val_float32(100.0),
                common::val_u32(5),
            ]),
            common::val_record(vec![
                common::val_string("G1001"),
                common::val_string("Golem Cloud Subscription 1y"),
                common::val_float32(999999.0),
                common::val_u32(1),
            ]),
            common::val_record(vec![
                common::val_string("G1002"),
                common::val_string("Mud Golem"),
                common::val_float32(11.0),
                common::val_u32(20),
            ])
        ])]
    )
}
