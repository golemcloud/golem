use crate::common;
use assert2::check;
use std::path::Path;

#[tokio::test]
async fn readwrite_get_returns_the_value_that_was_set() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-1";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![common::val_option(Some(common::val_list(vec![
                common::val_u8(1),
                common::val_u8(2),
                common::val_u8(3),
            ])))]
    );
}

#[tokio::test]
async fn readwrite_get_fails_if_the_value_was_not_set() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-2";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_option(None)]);
}

#[tokio::test]
async fn readwrite_set_replaces_the_value_if_it_was_already_set() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-3";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
                common::val_list(vec![
                    common::val_u8(4),
                    common::val_u8(5),
                    common::val_u8(6),
                ]),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![common::val_option(Some(common::val_list(vec![
                common::val_u8(4),
                common::val_u8(5),
                common::val_u8(6),
            ])))]
    );
}

#[tokio::test]
async fn readwrite_delete_removes_the_value_if_it_was_already_set() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-4";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/delete",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_option(None)]);
}

#[tokio::test]
async fn readwrite_exists_returns_true_if_the_value_was_set() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-5";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/exists",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_bool(true)]);
}

#[tokio::test]
async fn readwrite_exists_returns_false_if_the_value_was_not_set() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-6";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/exists",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_bool(false)]);
}

#[tokio::test]
async fn readwrite_buckets_can_be_shared_between_workers() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_id_1 = executor
        .start_worker(&template_id, "key-value-service-7")
        .await;
    let worker_id_2 = executor
        .start_worker(&template_id, "key-value-service-8")
        .await;

    let _ = executor
        .invoke_and_await(
            &worker_id_1,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-bucket")),
                common::val_string("key"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id_2,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-bucket")),
                common::val_string("key"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![common::val_option(Some(common::val_list(vec![
                common::val_u8(1),
                common::val_u8(2),
                common::val_u8(3),
            ])))]
    );
}

#[tokio::test]
async fn batch_get_many_gets_multiple_values() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-9";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key1"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key2"),
                common::val_list(vec![
                    common::val_u8(4),
                    common::val_u8(5),
                    common::val_u8(6),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key3"),
                common::val_list(vec![
                    common::val_u8(7),
                    common::val_u8(8),
                    common::val_u8(9),
                ]),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-many",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_list(vec![
                    common::val_string("key1"),
                    common::val_string("key2"),
                    common::val_string("key3"),
                ]),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![common::val_option(Some(common::val_list(vec![
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
                common::val_list(vec![
                    common::val_u8(4),
                    common::val_u8(5),
                    common::val_u8(6),
                ]),
                common::val_list(vec![
                    common::val_u8(7),
                    common::val_u8(8),
                    common::val_u8(9),
                ])
            ])))]
    );
}

#[tokio::test]
async fn batch_get_many_fails_if_any_value_was_not_set() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-10";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key1"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key2"),
                common::val_list(vec![
                    common::val_u8(4),
                    common::val_u8(5),
                    common::val_u8(6),
                ]),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-many",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_list(vec![
                    common::val_string("key1"),
                    common::val_string("key2"),
                    common::val_string("key3"),
                ]),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result == vec![common::val_option(None)]);
}

#[tokio::test]
async fn batch_set_many_sets_multiple_values() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-11";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set-many",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_list(vec![
                    common::val_pair(
                        common::val_string("key1"),
                        common::val_list(vec![
                            common::val_u8(1),
                            common::val_u8(2),
                            common::val_u8(3),
                        ]),
                    ),
                    common::val_pair(
                        common::val_string("key2"),
                        common::val_list(vec![
                            common::val_u8(4),
                            common::val_u8(5),
                            common::val_u8(6),
                        ]),
                    ),
                    common::val_pair(
                        common::val_string("key3"),
                        common::val_list(vec![
                            common::val_u8(7),
                            common::val_u8(8),
                            common::val_u8(9),
                        ]),
                    ),
                ]),
            ],
        )
        .await
        .unwrap();

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key1"),
            ],
        )
        .await
        .unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key2"),
            ],
        )
        .await
        .unwrap();

    let result3 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key3"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result1
            == vec![common::val_option(Some(common::val_list(vec![
                common::val_u8(1),
                common::val_u8(2),
                common::val_u8(3),
            ])))]
    );
    check!(
        result2
            == vec![common::val_option(Some(common::val_list(vec![
                common::val_u8(4),
                common::val_u8(5),
                common::val_u8(6),
            ])))]
    );
    check!(
        result3
            == vec![common::val_option(Some(common::val_list(vec![
                common::val_u8(7),
                common::val_u8(8),
                common::val_u8(9),
            ])))]
    );
}

#[tokio::test]
async fn batch_delete_many_deletes_multiple_values() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-12";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key1"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key2"),
                common::val_list(vec![
                    common::val_u8(4),
                    common::val_u8(5),
                    common::val_u8(6),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key3"),
                common::val_list(vec![
                    common::val_u8(7),
                    common::val_u8(8),
                    common::val_u8(9),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/delete-many",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_list(vec![
                    common::val_string("key1"),
                    common::val_string("key2"),
                    common::val_string("key3"),
                ]),
            ],
        )
        .await
        .unwrap();

    let result1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key1"),
            ],
        )
        .await
        .unwrap();

    let result2 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key2"),
            ],
        )
        .await
        .unwrap();

    let result3 = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key3"),
            ],
        )
        .await
        .unwrap();

    drop(executor);

    check!(result1 == vec![common::val_option(None)]);
    check!(result2 == vec![common::val_option(None)]);
    check!(result3 == vec![common::val_option(None)]);
}

#[tokio::test]
async fn batch_get_keys_returns_multiple_keys() {
    let mut executor = common::start().await.unwrap();

    let template_id =
        executor.store_template(Path::new("../test-templates/key-value-service.wasm"));
    let worker_name = "key-value-service-13";
    let worker_id = executor.start_worker(&template_id, worker_name).await;

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key1"),
                common::val_list(vec![
                    common::val_u8(1),
                    common::val_u8(2),
                    common::val_u8(3),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key2"),
                common::val_list(vec![
                    common::val_u8(4),
                    common::val_u8(5),
                    common::val_u8(6),
                ]),
            ],
        )
        .await
        .unwrap();

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/set",
            vec![
                common::val_string(&format!("{template_id}-{worker_name}-bucket")),
                common::val_string("key3"),
                common::val_list(vec![
                    common::val_u8(7),
                    common::val_u8(8),
                    common::val_u8(9),
                ]),
            ],
        )
        .await
        .unwrap();

    let result = executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api/get-keys",
            vec![common::val_string(&format!(
                "{template_id}-{worker_name}-bucket"
            ))],
        )
        .await
        .unwrap();

    drop(executor);

    check!(
        result
            == vec![common::val_list(vec![
                common::val_string("key1"),
                common::val_string("key2"),
                common::val_string("key3"),
            ])]
    );
}
