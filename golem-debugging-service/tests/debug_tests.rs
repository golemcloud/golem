use crate::debug_mode::debug_worker_executor::UntypedJrpcMessage;
use crate::*;
use golem_common::model::oplog::public_oplog_entry::ExportedFunctionCompletedParams;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{Timestamp, WorkerId};
use golem_debugging_service::model::params::PlaybackOverride;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::analysis::analysed_type::{record, str, variant};
use golem_wasm::analysis::{NameOptionTypePair, NameTypePair};
use golem_wasm::{IntoValueAndType, Record, Value, ValueAndType};
use golem_worker_executor_test_utils::{LastUniqueId, TestContext, TestWorkerExecutor};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

// Loading an existing worker (in regular executor) into debug mode (into debug service)
#[test]
#[tracing::instrument]
async fn test_connect_non_invoked_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await
        .unwrap_or_else(|e| panic!("Failed to start a regular worker: {e}"));

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    drop(regular_worker_executor);

    assert_eq!(connect_result.worker_id, worker_id);

    Ok(())
}

// Loading an existing worker (in regular executor) into debug mode (into debug service)
#[test]
#[tracing::instrument]
async fn test_connect_invoked_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    regular_worker_executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await??;

    regular_worker_executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    let connect_result = debug_executor.connect(&worker_id).await?;

    assert_eq!(connect_result.worker_id, worker_id);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_connect_and_playback(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?;

    let connect_result = debug_executor.connect(&worker_id).await?;

    let first_invocation_boundary = nth_invocation_boundary(&oplogs, 1);

    let playback_result = debug_executor
        .playback(first_invocation_boundary, None)
        .await?;

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert_eq!(playback_result.current_index, first_invocation_boundary);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_connect_and_playback_raw(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await?;

    regular_worker_executor
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem T-Shirt L".into_value_and_type()),
                ("price", 120.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    regular_worker_executor
        .invoke_and_await(&worker_id, "golem:it/api.{checkout}", vec![])
        .await??;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?;

    debug_executor.connect(&worker_id).await?;

    let first_invocation_boundary = nth_invocation_boundary(&oplogs, 1);
    let fourth_invocation_boundary = nth_invocation_boundary(&oplogs, 4);

    let playback_result_1 = debug_executor
        .playback(fourth_invocation_boundary, None)
        .await?;

    assert!(!playback_result_1.incremental_playback);

    debug_executor.rewind(first_invocation_boundary).await?;

    let playback_result_2 = debug_executor
        .playback(fourth_invocation_boundary, None)
        .await?;

    assert!(playback_result_2.incremental_playback);

    let all_messages = debug_executor.all_read_messages();

    assert!(matches!(
        &all_messages[..],
        [
            UntypedJrpcMessage {
                result: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                result: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                result: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                method: Some(_),
                ..
            },
            UntypedJrpcMessage {
                result: Some(_),
                ..
            },
        ]
    ));

    for id in [1, 2, 3, 4, 6, 8, 9, 10] {
        assert_eq!(all_messages[id].method, Some("emit-logs".to_string()))
    }

    assert_eq!(
        all_messages[1].params.clone().unwrap().as_array().unwrap()[0]
            .as_object()
            .unwrap()
            .get("message")
            .unwrap(),
        &serde_json::json!("Initializing cart for user test-user-1\n")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_connect_and_playback_to_middle_of_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?;

    let connect_result = debug_executor.connect(&worker_id).await?;

    let first_invocation_boundary = nth_invocation_boundary(&oplogs, 1);

    let index_in_middle = previous_index(first_invocation_boundary);

    let playback_result = debug_executor.playback(index_in_middle, None).await?;

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert_eq!(playback_result.current_index, first_invocation_boundary); // Playback should stop at the end of the invocation

    Ok(())
}

// playback twice with progressing indexes
#[test]
#[tracing::instrument]
async fn test_playback_from_breakpoint(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?;

    let initialize_boundary = nth_invocation_boundary(&oplogs, 1);

    let add_item_boundary = nth_invocation_boundary(&oplogs, 2);

    let connect_result = debug_executor.connect(&worker_id).await?;

    let playback_result1 = debug_executor.playback(initialize_boundary, None).await?;

    let current_index = debug_executor.current_index().await?;

    // In the test We expect the current index to be the index of the initialize-cart invocation within
    // the configured wait_time_in_seconds.
    // If this is false, we haven't waited enough. Asking for the current index again
    // will be a good way to ensure that the playback has completed.
    assert_eq!(current_index, initialize_boundary);

    let playback_result2 = debug_executor.playback(add_item_boundary, None).await?;

    let current_index = debug_executor.current_index().await?;

    assert_eq!(current_index, add_item_boundary);

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result1.worker_id, worker_id);
    assert_eq!(playback_result1.current_index, initialize_boundary);
    assert!(!playback_result1.incremental_playback);
    assert_eq!(playback_result2.worker_id, worker_id);
    assert!(playback_result2.incremental_playback);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_playback_and_rewind(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?;

    let first_boundary = nth_invocation_boundary(&oplogs, 1);

    let second_boundary = nth_invocation_boundary(&oplogs, 2);

    let connect_result = debug_executor.connect(&worker_id).await?;

    let playback_result = debug_executor.playback(second_boundary, None).await?;

    let rewind_result = debug_executor.rewind(first_boundary).await?;

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert_eq!(playback_result.current_index, second_boundary);
    assert_eq!(rewind_result.worker_id, worker_id);
    assert_eq!(rewind_result.current_index, first_boundary);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_playback_and_fork(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let first_boundary = nth_invocation_boundary(&oplogs, 1);

    let connect_result = debug_executor.connect(&worker_id).await?;

    let playback_result = debug_executor.playback(first_boundary, None).await?;

    let target_worker_id = WorkerId {
        component_id: worker_id.component_id.clone(),
        worker_name: "forked-worker".to_string(),
    };

    let fork_result = debug_executor.fork(&target_worker_id, first_boundary).await;

    // We ensure first invocation exists in the forked worker
    let first_invocation_in_forked = regular_worker_executor
        .search_oplog(&target_worker_id, "initialize-cart")
        .await?;

    // We ensure second invocation doesn't exist in the forked worker
    let second_invocation_in_forked = regular_worker_executor
        .search_oplog(&target_worker_id, "G1000")
        .await?;

    // invoke the forked worker with second invocation
    regular_worker_executor
        .invoke_and_await(
            &target_worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    let post_fork_invocation_in_forked = regular_worker_executor
        .search_oplog(&target_worker_id, "G1000")
        .await?;

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert!(fork_result.is_ok());
    assert!(!first_invocation_in_forked.is_empty());
    assert!(second_invocation_in_forked.is_empty());
    assert!(!post_fork_invocation_in_forked.is_empty());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_playback_with_overrides(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(&context.default_environment_id, "shopping-cart")
        .store()
        .await?;

    let worker_id = regular_worker_executor
        .start_worker(&component.id, "shopping-cart")
        .await?;

    let shopping_cart_execution_result =
        run_shopping_cart_workflow(&regular_worker_executor, &worker_id).await?;

    let new_checkout_result = new_shopping_cart_checkout_result();

    let public_oplog_entry =
        PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParams {
            timestamp: Timestamp::now_utc(),
            response: Some(new_checkout_result),
            consumed_fuel: 0,
        });

    let oplog_overrides = PlaybackOverride {
        index: shopping_cart_execution_result.last_checkout_boundary,
        oplog: public_oplog_entry,
    };

    // Connect in Debug mode
    debug_executor.connect(&worker_id).await?;

    // Playback until the last invocation boundary
    debug_executor
        .playback(
            shopping_cart_execution_result.last_add_item_boundary,
            Some(vec![oplog_overrides]),
        )
        .await?;

    // Fork from the last checkout boundary
    let target_worker_id = WorkerId {
        component_id: worker_id.component_id.clone(),
        worker_name: "forked-worker".to_string(),
    };

    // While we played back until last boundary, we fork from shopping cart boundary
    let fork = debug_executor
        .fork(
            &target_worker_id,
            shopping_cart_execution_result.last_checkout_boundary,
        )
        .await;

    assert!(fork.is_ok());

    // Oplogs in forked worker in regular executor
    let oplogs_in_forked_worker = regular_worker_executor
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;

    let entry = oplogs_in_forked_worker.last();

    if let Some(PublicOplogEntryWithIndex {
        entry: PublicOplogEntry::ExportedFunctionCompleted(completed),
        oplog_index: _,
    }) = entry
    {
        assert_eq!(
            completed.response,
            Some(new_shopping_cart_checkout_result())
        );
    } else {
        panic!("Expected ExportedFunctionCompleted entry");
    }

    assert_eq!(
        oplogs_in_forked_worker.len(),
        u64::from(shopping_cart_execution_result.last_checkout_boundary) as usize
    );

    Ok(())
}

fn nth_invocation_boundary(oplogs: &[PublicOplogEntryWithIndex], n: usize) -> OplogIndex {
    let index = oplogs
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(&entry.entry, PublicOplogEntry::ExportedFunctionCompleted(_)))
        .nth(n - 1)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("No {n}th invocation boundary found"));

    OplogIndex::from_u64((index + 1) as u64)
}

fn previous_index(index: OplogIndex) -> OplogIndex {
    OplogIndex::from_u64(u64::from(index) - 1)
}

// Regardless of the items added, we override the checkout result
// with a new value.
fn new_shopping_cart_checkout_result() -> ValueAndType {
    ValueAndType::new(
        Value::Variant {
            case_idx: 1,
            case_value: Some(Box::new(Value::Record(vec![Value::String(
                "new-order-id".to_string(),
            )]))),
        },
        variant(vec![
            NameOptionTypePair {
                name: "error".to_string(),
                typ: Some(str()),
            },
            NameOptionTypePair {
                name: "success".to_string(),
                typ: Some(record(vec![NameTypePair {
                    name: "order-id".to_string(),
                    typ: str(),
                }])),
            },
        ]),
    )
}

async fn run_shopping_cart_initialize_and_add(
    regular_worker_executor: &TestWorkerExecutor,
    worker_id: &WorkerId,
) -> anyhow::Result<()> {
    regular_worker_executor
        .invoke_and_await(
            worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await??;

    regular_worker_executor
        .invoke_and_await(
            worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    Ok(())
}

async fn run_shopping_cart_workflow(
    regular_worker_executor: &TestWorkerExecutor,
    worker_id: &WorkerId,
) -> anyhow::Result<ShoppingCartExecutionResult> {
    // Initialize
    run_shopping_cart_initialize_and_add(regular_worker_executor, worker_id).await?;

    // Checkout
    regular_worker_executor
        .invoke_and_await(worker_id, "golem:it/api.{checkout}", vec![])
        .await??;

    // Add Item again
    regular_worker_executor
        .invoke_and_await(
            worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await??;

    let oplogs = regular_worker_executor
        .get_oplog(worker_id, OplogIndex::INITIAL)
        .await?;

    let last_checkout_boundary = nth_invocation_boundary(&oplogs, 3);
    let last_add_item_boundary = nth_invocation_boundary(&oplogs, 4);

    Ok(ShoppingCartExecutionResult {
        last_checkout_boundary,
        last_add_item_boundary,
    })
}

struct ShoppingCartExecutionResult {
    last_checkout_boundary: OplogIndex,
    last_add_item_boundary: OplogIndex,
}
