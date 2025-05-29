use crate::debug_mode::debug_worker_executor::DebugWorkerExecutorClient;
use crate::regular_mode::regular_worker_executor::TestRegularWorkerExecutor;
use crate::*;
use cloud_debugging_service::model::params::PlaybackOverride;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::{ExportedFunctionCompletedParameters, PublicOplogEntry};
use golem_common::model::{Timestamp, WorkerId};
use golem_test_framework::dsl::TestDsl;
use golem_wasm_ast::analysis::analysed_type::{record, str, variant};
use golem_wasm_ast::analysis::{NameOptionTypePair, NameTypePair};
use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(RegularWorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

// Loading an existing worker (in regular executor) into debug mode (into debug service)
#[test]
#[tracing::instrument]
async fn test_connect_non_invoked_worker(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);

    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .start_worker(&component, "shopping-cart")
        .await
        .unwrap_or_else(|e| panic!("Failed to start a regular worker: {}", e));

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    drop(regular_worker_executor);

    assert_eq!(connect_result.worker_id, worker_id);
}

// Loading an existing worker (in regular executor) into debug mode (into debug service)
#[test]
#[tracing::instrument]
async fn test_connect_invoked_worker(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);
    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .try_start_worker(&component, "shopping-cart")
        .await
        .unwrap()
        .unwrap();

    let _ = regular_worker_executor
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = regular_worker_executor
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    drop(regular_worker_executor);

    assert_eq!(connect_result.worker_id, worker_id);
}

#[test]
#[tracing::instrument]
async fn test_connect_and_playback(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);
    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .start_worker(&component, "shopping-cart")
        .await
        .unwrap();

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    let first_invocation_boundary = nth_invocation_boundary(&oplogs, 1);

    let playback_result = debug_executor
        .playback(first_invocation_boundary, None, 3)
        .await
        .expect("Failed to playback the worker in debug mode");

    drop(regular_worker_executor);

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert_eq!(playback_result.current_index, first_invocation_boundary);
}

#[test]
#[tracing::instrument]
async fn test_connect_and_playback_to_middle_of_invocation(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);
    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .start_worker(&component, "shopping-cart")
        .await
        .unwrap();

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    let first_invocation_boundary = nth_invocation_boundary(&oplogs, 1);

    let index_in_middle = previous_index(first_invocation_boundary);

    let playback_result = debug_executor
        .playback(index_in_middle, None, 3)
        .await
        .expect("Failed to playback the worker in debug mode");

    drop(regular_worker_executor);

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert_eq!(playback_result.current_index, first_invocation_boundary); // Playback should stop at the end of the invocation
}

// playback twice with progressing indexes
#[test]
#[tracing::instrument]
async fn test_playback_from_breakpoint(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);
    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .try_start_worker(&component, "shopping-cart")
        .await
        .unwrap()
        .unwrap();

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let initialize_boundary = nth_invocation_boundary(&oplogs, 1);

    let add_item_boundary = nth_invocation_boundary(&oplogs, 2);

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    let playback_result1 = debug_executor
        .playback(initialize_boundary, None, 3)
        .await
        .expect("Failed to playback the worker in debug mode");

    let current_index = debug_executor
        .current_index()
        .await
        .expect("Failed to get current index");

    // In the test We expect the current index to be the index of the initialize-cart invocation within
    // the configured wait_time_in_seconds.
    // If this is false, we haven't waited enough. Asking for the current index again
    // will be a good way to ensure that the playback has completed.
    assert_eq!(current_index, initialize_boundary);

    let playback_result2 = debug_executor
        .playback(add_item_boundary, None, 3)
        .await
        .expect("Failed to playback the worker in debug mode");

    let current_index = debug_executor
        .current_index()
        .await
        .expect("Failed to get current index");

    assert_eq!(current_index, add_item_boundary);

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result1.worker_id, worker_id);
    assert_eq!(playback_result1.current_index, initialize_boundary);
    assert_eq!(playback_result2.worker_id, worker_id);
}

#[test]
#[tracing::instrument]
async fn test_playback_and_rewind(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);
    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .try_start_worker(&component, "shopping-cart")
        .await
        .unwrap()
        .unwrap();

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let first_boundary = nth_invocation_boundary(&oplogs, 1);

    let second_boundary = nth_invocation_boundary(&oplogs, 2);

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    let playback_result = debug_executor
        .playback(second_boundary, None, 3)
        .await
        .expect("Failed to playback the worker in debug mode");

    let rewind_result = debug_executor
        .rewind(first_boundary, 3)
        .await
        .expect("Failed to playback the worker in debug mode");

    drop(regular_worker_executor);

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert_eq!(playback_result.current_index, second_boundary);
    assert_eq!(rewind_result.worker_id, worker_id);
    assert_eq!(rewind_result.current_index, first_boundary);
}

#[test]
#[tracing::instrument]
async fn test_playback_and_fork(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);
    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .try_start_worker(&component, "shopping-cart")
        .await
        .unwrap()
        .unwrap();

    run_shopping_cart_initialize_and_add(&regular_worker_executor, &worker_id).await;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let first_boundary = nth_invocation_boundary(&oplogs, 1);

    let connect_result = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    let playback_result = debug_executor
        .playback(first_boundary, None, 3)
        .await
        .expect("Failed to playback the worker in debug mode");

    let target_worker_id = WorkerId {
        component_id: worker_id.component_id.clone(),
        worker_name: "forked-worker".to_string(),
    };

    let fork_result = debug_executor.fork(&target_worker_id, first_boundary).await;

    // We ensure first invocation exists in the forked worker
    let first_invocation_in_forked = regular_worker_executor
        .search_oplog(&target_worker_id, "initialize-cart")
        .await
        .expect("Failed to search oplog for first invocation");

    // We ensure second invocation doesn't exist in the forked worker
    let second_invocation_in_forked = regular_worker_executor
        .search_oplog(&target_worker_id, "G1000")
        .await
        .expect("Failed to search oplog for second invocation");

    // invoke the forked worker with second invocation
    let _ = regular_worker_executor
        .invoke_and_await(
            target_worker_id.clone(),
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let post_fork_invocation_in_forked = regular_worker_executor
        .search_oplog(&target_worker_id, "G1000")
        .await
        .expect("Failed to search oplog for second invocation");

    drop(regular_worker_executor);

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert!(fork_result.is_ok());
    assert!(!first_invocation_in_forked.is_empty());
    assert!(second_invocation_in_forked.is_empty());
    assert!(!post_fork_invocation_in_forked.is_empty());
}

#[test]
#[tracing::instrument]
async fn test_playback_with_overrides(
    last_unique_id: &LastUniqueId,
    deps: &RegularWorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    let context = RegularExecutorTestContext::new(last_unique_id);
    let regular_worker_executor = start_regular_executor(deps, &context).await;

    let debug_context = DebugExecutorTestContext::from(&context);
    let mut debug_executor = start_debug_executor(deps, &debug_context).await;

    let component = regular_worker_executor
        .component("shopping-cart")
        .store()
        .await;

    let worker_id = regular_worker_executor
        .try_start_worker(&component, "shopping-cart")
        .await
        .unwrap()
        .unwrap();

    let shopping_cart_execution_result =
        run_shopping_cart_workflow(&regular_worker_executor, &worker_id).await;

    let new_checkout_result = new_shopping_cart_checkout_result();

    let public_oplog_entry =
        PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
            timestamp: Timestamp::now_utc(),
            response: new_checkout_result,
            consumed_fuel: 0,
        });

    let oplog_overrides = PlaybackOverride {
        index: shopping_cart_execution_result.last_checkout_boundary,
        oplog: public_oplog_entry,
    };

    // Connect in Debug mode
    let _ = debug_executor
        .connect(&worker_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    // Playback until the last invocation boundary
    let _ = debug_executor
        .playback(
            shopping_cart_execution_result.last_add_item_boundary,
            Some(vec![oplog_overrides]),
            3,
        )
        .await
        .expect("Failed to playback the worker in debug mode");

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
        .await
        .unwrap();

    let entry = oplogs_in_forked_worker.last();

    if let Some(PublicOplogEntry::ExportedFunctionCompleted(completed)) = entry {
        assert_eq!(completed.response, new_shopping_cart_checkout_result());
    } else {
        panic!("Expected ExportedFunctionCompleted entry");
    }

    drop(regular_worker_executor);

    assert_eq!(
        oplogs_in_forked_worker.len(),
        u64::from(shopping_cart_execution_result.last_checkout_boundary) as usize
    );
}

fn nth_invocation_boundary(oplogs: &[PublicOplogEntry], n: usize) -> OplogIndex {
    let index = oplogs
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, PublicOplogEntry::ExportedFunctionCompleted(_)))
        .nth(n - 1)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("No {}th invocation boundary found", n));

    OplogIndex::from_u64((index + 1) as u64)
}

async fn start_regular_executor(
    deps: &RegularWorkerExecutorTestDependencies,
    context: &RegularExecutorTestContext,
) -> TestRegularWorkerExecutor {
    start_regular_worker_executor(deps, context)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Failed to start regular executor at port {}: {}",
                context.grpc_port(),
                e
            )
        })
}

async fn start_debug_executor(
    deps: &RegularWorkerExecutorTestDependencies,
    context: &DebugExecutorTestContext,
) -> DebugWorkerExecutorClient {
    start_debug_worker_executor(deps, context)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Failed to start debug executor at port {}: {}",
                context.debug_server_port(),
                e
            )
        })
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
    regular_worker_executor: &TestRegularWorkerExecutor,
    worker_id: &WorkerId,
) {
    regular_worker_executor
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await
        .unwrap()
        .expect("Failed to invoke and await");

    regular_worker_executor
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await
        .unwrap()
        .expect("Failed to invoke and await");
}

async fn run_shopping_cart_workflow(
    regular_worker_executor: &TestRegularWorkerExecutor,
    worker_id: &WorkerId,
) -> ShoppingCartExecutionResult {
    // Initialize
    run_shopping_cart_initialize_and_add(regular_worker_executor, worker_id).await;

    // Checkout
    let _ = regular_worker_executor
        .invoke_and_await(worker_id.clone(), "golem:it/api.{checkout}", vec![])
        .await
        .expect("Failed to invoke and await");

    // Add Item again
    let _ = regular_worker_executor
        .invoke_and_await(
            worker_id.clone(),
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let oplogs = regular_worker_executor
        .get_oplog(worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let last_checkout_boundary = nth_invocation_boundary(&oplogs, 3);
    let last_add_item_boundary = nth_invocation_boundary(&oplogs, 4);

    ShoppingCartExecutionResult {
        last_checkout_boundary,
        last_add_item_boundary,
    }
}

struct ShoppingCartExecutionResult {
    last_checkout_boundary: OplogIndex,
    last_add_item_boundary: OplogIndex,
}
