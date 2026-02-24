use crate::*;
use golem_common::model::agent::AgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::public_oplog_entry::AgentInvocationFinishedParams;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{Timestamp, WorkerId};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_debugging_service::model::params::PlaybackOverride;
use golem_test_framework::dsl::TestDsl;
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "non-invoked");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "invoked");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    regular_worker_executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    regular_worker_executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "playback");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "playback-raw");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    regular_worker_executor
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    regular_worker_executor
        .invoke_and_await_agent(&component, &repo_id, "list", data_value!())
        .await?;

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

    assert_eq!(
        all_messages.iter().filter(|m| m.result.is_some()).count(),
        4
    );

    let emit_log_indices: Vec<usize> = all_messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            if m.method == Some("emit-logs".to_string()) {
                Some(i)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(emit_log_indices, vec![1, 2, 3, 6, 7, 8]);

    assert_eq!(
        all_messages[1].params.clone().unwrap().as_array().unwrap()[0]
            .as_object()
            .unwrap()
            .get("message")
            .unwrap(),
        &serde_json::json!("Adding item G1000 to repository\n")
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "playback-middle");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "breakpoint");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?;

    let first_add_boundary = nth_invocation_boundary(&oplogs, 1);

    let second_add_boundary = nth_invocation_boundary(&oplogs, 2);

    let connect_result = debug_executor.connect(&worker_id).await?;

    let playback_result1 = debug_executor.playback(first_add_boundary, None).await?;

    let current_index = debug_executor.current_index().await?;

    assert_eq!(current_index, first_add_boundary);

    let playback_result2 = debug_executor.playback(second_add_boundary, None).await?;

    let current_index = debug_executor.current_index().await?;

    assert_eq!(current_index, second_add_boundary);

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result1.worker_id, worker_id);
    assert_eq!(playback_result1.current_index, first_add_boundary);
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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "rewind");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "fork-source");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let first_boundary = nth_invocation_boundary(&oplogs, 1);

    let connect_result = debug_executor.connect(&worker_id).await?;

    let playback_result = debug_executor.playback(first_boundary, None).await?;

    let forked_repo_id = agent_id!("repository", "forked-worker");
    let target_worker_id = WorkerId::from_agent_id(worker_id.component_id, &forked_repo_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let fork_result = debug_executor.fork(&target_worker_id, first_boundary).await;

    // Verify forked worker has oplog entries up to first boundary (first add only)
    let forked_oplogs = regular_worker_executor
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;
    let forked_oplog_len_before = forked_oplogs.len();

    // Invoke the forked worker with another add
    regular_worker_executor
        .invoke_and_await_agent(
            &component,
            &forked_repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    let forked_oplogs_after = regular_worker_executor
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;

    assert_eq!(connect_result.worker_id, worker_id);
    assert_eq!(playback_result.worker_id, worker_id);
    assert!(fork_result.is_ok());
    assert_eq!(forked_oplog_len_before, u64::from(first_boundary) as usize);
    assert!(forked_oplogs_after.len() > forked_oplog_len_before);

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
        .component(&context.default_environment_id, "it_agent_counters_release")
        .name("it:agent-counters")
        .store()
        .await?;

    let repo_id = agent_id!("repository", "overrides");
    let worker_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    let workflow_result = run_repo_workflow(&regular_worker_executor, &component, &repo_id).await?;

    // Extract the actual response from the list invocation to get the correct type,
    // then construct a modified override value with the same type structure
    let original_list_entry =
        &workflow_result.oplogs[u64::from(workflow_result.list_boundary) as usize - 1];
    let original_result = match &original_list_entry.entry {
        PublicOplogEntry::AgentInvocationFinished(completed) => completed.result.clone(),
        _ => panic!("Expected AgentInvocationFinished at list boundary"),
    };

    let public_oplog_entry =
        PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
            timestamp: Timestamp::now_utc(),
            result: original_result.clone(),
            consumed_fuel: 0,
            component_revision: 0,
        });

    let oplog_overrides = PlaybackOverride {
        index: workflow_result.list_boundary,
        oplog: public_oplog_entry,
    };

    // Connect in Debug mode
    debug_executor.connect(&worker_id).await?;

    // Playback until the last invocation boundary
    debug_executor
        .playback(
            workflow_result.last_add_boundary,
            Some(vec![oplog_overrides]),
        )
        .await?;

    // Fork from the list boundary
    let target_agent_id = phantom_agent_id!("repository", uuid::Uuid::new_v4(), "overrides");
    let target_worker_id = WorkerId::from_agent_id(worker_id.component_id, &target_agent_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let fork = debug_executor
        .fork(&target_worker_id, workflow_result.list_boundary)
        .await;

    assert!(fork.is_ok());

    // Oplogs in forked worker in regular executor
    let oplogs_in_forked_worker = regular_worker_executor
        .get_oplog(&target_worker_id, OplogIndex::INITIAL)
        .await?;

    let entry = oplogs_in_forked_worker.last();

    if let Some(PublicOplogEntryWithIndex {
        entry: PublicOplogEntry::AgentInvocationFinished(completed),
        oplog_index: _,
    }) = entry
    {
        assert_eq!(completed.result, original_result);
    } else {
        panic!("Expected AgentInvocationFinished entry");
    }

    assert_eq!(
        oplogs_in_forked_worker.len(),
        u64::from(workflow_result.list_boundary) as usize
    );

    Ok(())
}

fn nth_invocation_boundary(oplogs: &[PublicOplogEntryWithIndex], n: usize) -> OplogIndex {
    let index = oplogs
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .nth(n - 1)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("No {n}th invocation boundary found"));

    OplogIndex::from_u64((index + 1) as u64)
}

fn previous_index(index: OplogIndex) -> OplogIndex {
    OplogIndex::from_u64(u64::from(index) - 1)
}

async fn run_repo_add_two(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    repo_id: &AgentId,
) -> anyhow::Result<()> {
    executor
        .invoke_and_await_agent(
            component,
            repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    executor
        .invoke_and_await_agent(
            component,
            repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

    Ok(())
}

async fn run_repo_workflow(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    repo_id: &AgentId,
) -> anyhow::Result<RepoWorkflowResult> {
    // Add two items
    run_repo_add_two(executor, component, repo_id).await?;

    // List
    executor
        .invoke_and_await_agent(component, repo_id, "list", data_value!())
        .await?;

    // Add another item
    executor
        .invoke_and_await_agent(component, repo_id, "add", data_value!("G1002", "Mud Golem"))
        .await?;

    let worker_id =
        WorkerId::from_agent_id(component.id, repo_id).map_err(|e| anyhow::anyhow!("{e}"))?;

    let oplogs = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let list_boundary = nth_invocation_boundary(&oplogs, 3);
    let last_add_boundary = nth_invocation_boundary(&oplogs, 4);

    Ok(RepoWorkflowResult {
        oplogs,
        list_boundary,
        last_add_boundary,
    })
}

struct RepoWorkflowResult {
    oplogs: Vec<PublicOplogEntryWithIndex>,
    list_boundary: OplogIndex,
    last_add_boundary: OplogIndex,
}
