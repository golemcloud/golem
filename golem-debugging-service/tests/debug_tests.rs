use crate::debug_mode::debug_worker_executor::DebugWorkerExecutorClient;
use crate::*;
use golem_common::base_model::component::ComponentRevision;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::public_oplog_entry::StartParams;
use golem_common::model::oplog::public_oplog_entry::{
    AgentInvocationFinishedParams, EndParams, NoOpParams,
};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::{AgentId, Timestamp};
use golem_common::{agent_id, data_value, phantom_agent_id};
use golem_debugging_service::model::params::PlaybackOverride;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{LastUniqueId, TestContext, TestWorkerExecutor};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

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

    let repo_id = agent_id!("Repository", "non-invoked");
    let agent_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await
        .unwrap_or_else(|e| panic!("Failed to start a regular worker: {e}"));

    let connect_result = debug_executor
        .connect(&agent_id)
        .await
        .expect("Failed to connect to the worker in debug mode");

    drop(regular_worker_executor);

    assert_eq!(connect_result.agent_id, agent_id);

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

    let repo_id = agent_id!("Repository", "invoked");
    let agent_id = regular_worker_executor
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

    let connect_result = debug_executor.connect(&agent_id).await?;

    assert_eq!(connect_result.agent_id, agent_id);

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

    let repo_id = agent_id!("Repository", "playback");
    let agent_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;

    let connect_result = debug_executor.connect(&agent_id).await?;

    let first_invocation_boundary = nth_invocation_boundary(&oplogs, 1);

    let playback_result = debug_executor
        .playback(first_invocation_boundary, None)
        .await?;

    assert_eq!(connect_result.agent_id, agent_id);
    assert_eq!(playback_result.agent_id, agent_id);
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

    let repo_id = agent_id!("Repository", "playback-raw");
    let agent_id = regular_worker_executor
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
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;

    debug_executor.connect(&agent_id).await?;

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

    let repo_id = agent_id!("Repository", "playback-middle");
    let agent_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;

    let connect_result = debug_executor.connect(&agent_id).await?;

    let first_invocation_boundary = nth_invocation_boundary(&oplogs, 1);

    let index_in_middle = previous_index(first_invocation_boundary);

    let playback_result = debug_executor.playback(index_in_middle, None).await?;

    assert_eq!(connect_result.agent_id, agent_id);
    assert_eq!(playback_result.agent_id, agent_id);
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

    let repo_id = agent_id!("Repository", "breakpoint");
    let agent_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;

    let first_add_boundary = nth_invocation_boundary(&oplogs, 1);

    let second_add_boundary = nth_invocation_boundary(&oplogs, 2);

    let connect_result = debug_executor.connect(&agent_id).await?;

    let playback_result1 = debug_executor.playback(first_add_boundary, None).await?;

    let current_index = debug_executor.current_index().await?;

    assert_eq!(current_index, first_add_boundary);

    let playback_result2 = debug_executor.playback(second_add_boundary, None).await?;

    let current_index = debug_executor.current_index().await?;

    assert_eq!(current_index, second_add_boundary);

    assert_eq!(connect_result.agent_id, agent_id);
    assert_eq!(playback_result1.agent_id, agent_id);
    assert_eq!(playback_result1.current_index, first_add_boundary);
    assert!(!playback_result1.incremental_playback);
    assert_eq!(playback_result2.agent_id, agent_id);
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

    let repo_id = agent_id!("Repository", "rewind");
    let agent_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;

    let first_boundary = nth_invocation_boundary(&oplogs, 1);

    let second_boundary = nth_invocation_boundary(&oplogs, 2);

    let connect_result = debug_executor.connect(&agent_id).await?;

    let playback_result = debug_executor.playback(second_boundary, None).await?;

    let rewind_result = debug_executor.rewind(first_boundary).await?;

    assert_eq!(connect_result.agent_id, agent_id);
    assert_eq!(playback_result.agent_id, agent_id);
    assert_eq!(playback_result.current_index, second_boundary);
    assert_eq!(rewind_result.agent_id, agent_id);
    assert_eq!(rewind_result.current_index, first_boundary);

    Ok(())
}

#[test]
#[timeout("120s")]
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

    let repo_id = agent_id!("Repository", "fork-source");
    let agent_id = regular_worker_executor
        .start_agent(&component.id, repo_id.clone())
        .await?;

    run_repo_add_two(&regular_worker_executor, &component, &repo_id).await?;

    let oplogs = regular_worker_executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await
        .unwrap();

    let first_boundary = nth_invocation_boundary(&oplogs, 1);

    let connect_result = debug_executor.connect(&agent_id).await?;

    let playback_result = debug_executor.playback(first_boundary, None).await?;

    let forked_repo_id = agent_id!("Repository", "forked-worker");
    let target_agent_id = AgentId::from_agent_id(agent_id.component_id, &forked_repo_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let fork_result = debug_executor.fork(&target_agent_id, first_boundary).await;

    // Verify forked worker has oplog entries up to first boundary (first add only)
    let forked_oplogs = regular_worker_executor
        .get_oplog(&target_agent_id, OplogIndex::INITIAL)
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
        .get_oplog(&target_agent_id, OplogIndex::INITIAL)
        .await?;

    regular_worker_executor
        .check_oplog_is_queryable(&agent_id)
        .await?;
    regular_worker_executor
        .check_oplog_is_queryable(&target_agent_id)
        .await?;

    assert_eq!(connect_result.agent_id, agent_id);
    assert_eq!(playback_result.agent_id, agent_id);
    assert!(fork_result.is_ok());

    // The forked oplog should contain the entries up to first_boundary, plus
    // any CancelPendingInvocation/FailedUpdate entries appended by the fork to
    // cancel pending work inherited from the source worker's oplog.
    let cancel_count = forked_oplogs
        .iter()
        .filter(|e| {
            matches!(
                &e.entry,
                PublicOplogEntry::CancelPendingInvocation(_) | PublicOplogEntry::FailedUpdate(_)
            )
        })
        .count();
    assert_eq!(
        forked_oplog_len_before,
        u64::from(first_boundary) as usize + cancel_count
    );

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

    let repo_id = agent_id!("Repository", "overrides");
    let agent_id = regular_worker_executor
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
            method_name: None,
            consumed_fuel: 0,
            component_revision: ComponentRevision::INITIAL,
        });

    let oplog_overrides = PlaybackOverride {
        index: workflow_result.list_boundary,
        oplog: public_oplog_entry,
    };

    // Connect in Debug mode
    debug_executor.connect(&agent_id).await?;

    // Playback until the last invocation boundary
    debug_executor
        .playback(
            workflow_result.last_add_boundary,
            Some(vec![oplog_overrides]),
        )
        .await?;

    // Fork from the list boundary
    let target_agent_id = phantom_agent_id!("Repository", uuid::Uuid::new_v4(), "overrides");
    let target_agent_id = AgentId::from_agent_id(agent_id.component_id, &target_agent_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let fork = debug_executor
        .fork(&target_agent_id, workflow_result.list_boundary)
        .await;

    assert!(fork.is_ok());

    // Oplogs in forked worker in regular executor
    let oplogs_in_forked_worker = regular_worker_executor
        .get_oplog(&target_agent_id, OplogIndex::INITIAL)
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

// Debug playback over a worker whose recorded history contains concurrent P3 durable calls
// (`http::client::send` + a raced `monotonic_clock` sleep, both `Start`/terminal entry pairs).
// Playback to the invocation boundary must replay the completed durable calls from the oplog
// without re-executing the HTTP side effect.
#[test]
#[tracing::instrument]
async fn test_playback_over_p3_http_durable_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup = start_clock_debug_session(last_unique_id, deps, "playback-p3-full").await?;

    let (_send_start_index, send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);
    let boundary = invocation_boundary_after(&setup.oplogs, send_terminal_index);

    let requests_before = setup.request_count.load(Ordering::SeqCst);

    let playback_result = setup.debug_executor.playback(boundary, None).await?;

    assert_eq!(playback_result.agent_id, setup.agent_id);
    assert_eq!(playback_result.current_index, boundary);
    assert_eq!(
        setup.request_count.load(Ordering::SeqCst),
        requests_before,
        "debug playback must not re-execute the recorded HTTP call"
    );

    setup.server.abort();
    Ok(())
}

// Targeting a raw oplog index (`ensure_invocation_boundary=false`) that lies *on* a durable
// `Start` entry leaves that call without a terminal entry before the replay target. Debug
// sessions must refuse live repair / re-execution and surface an explicit error instead.
#[test]
#[tracing::instrument]
async fn test_playback_target_on_p3_start_refuses_live_repair(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup = start_clock_debug_session(last_unique_id, deps, "playback-p3-on-start").await?;

    let (send_start_index, _send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);

    let requests_before = setup.request_count.load(Ordering::SeqCst);
    let oplog_len_before = setup.oplogs.len();

    let playback_result = setup
        .debug_executor
        .playback_raw(send_start_index, None, false)
        .await;

    assert!(playback_result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("in-flight durable call"),
        "expected an explicit in-flight durable call error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );
    assert_eq!(
        setup.request_count.load(Ordering::SeqCst),
        requests_before,
        "refused playback must not re-execute the recorded HTTP call"
    );

    // The debug session must not have modified the real oplog
    let oplogs_after = setup
        .regular_worker_executor
        .get_oplog(&setup.agent_id, OplogIndex::INITIAL)
        .await?;
    assert_eq!(oplogs_after.len(), oplog_len_before);

    setup.server.abort();
    Ok(())
}

// Same as above, but the raw target lies strictly *inside* an in-flight durable call: past the
// `Start` of the raced `monotonic_clock` sleep but before its `Cancelled` terminal entry (the
// http send's `End` entry lies in that window).
#[test]
#[tracing::instrument]
async fn test_playback_target_inside_p3_call_refuses_live_repair(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup =
        start_clock_debug_session(last_unique_id, deps, "playback-p3-inside-call").await?;

    let (send_start_index, send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);
    assert!(send_terminal_index > send_start_index.next());

    let requests_before = setup.request_count.load(Ordering::SeqCst);

    let playback_result = setup
        .debug_executor
        .playback_raw(previous_index(send_terminal_index), None, false)
        .await;

    assert!(playback_result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("in-flight durable call"),
        "expected an explicit in-flight durable call error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );
    assert_eq!(
        setup.request_count.load(Ordering::SeqCst),
        requests_before,
        "refused playback must not re-execute the recorded HTTP call"
    );

    setup.server.abort();
    Ok(())
}

// A raw target *after* all terminal entries of the invocation's durable calls is safe: every
// recorded durable call resolves completely from the oplog, so playback succeeds without
// re-executing side effects.
#[test]
#[tracing::instrument]
async fn test_playback_target_after_p3_terminals_succeeds(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup =
        start_clock_debug_session(last_unique_id, deps, "playback-p3-after-call").await?;

    let (_send_start_index, send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);
    let boundary = invocation_boundary_after(&setup.oplogs, send_terminal_index);
    let last_terminal_index = last_durable_terminal_before(&setup.oplogs, boundary);
    assert!(last_terminal_index < boundary);

    let requests_before = setup.request_count.load(Ordering::SeqCst);

    let playback_result = setup
        .debug_executor
        .playback_raw(last_terminal_index, None, false)
        .await?;

    // Playback replays every recorded durable call from the oplog and stops at the raw target
    // (the worker suspends as soon as it would run live past it)
    assert_eq!(playback_result.current_index, last_terminal_index);
    assert_eq!(
        setup.request_count.load(Ordering::SeqCst),
        requests_before,
        "debug playback must not re-execute the recorded HTTP call"
    );

    setup.server.abort();
    Ok(())
}

// Playback overrides that would break the `Start`/terminal-entry pairing of the recorded oplog
// must be rejected by validation; pairing-preserving overrides are accepted.
#[test]
#[tracing::instrument]
async fn test_playback_override_pairing_validation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup =
        start_clock_debug_session(last_unique_id, deps, "playback-p3-overrides").await?;

    let (send_start_index, send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);
    let boundary = invocation_boundary_after(&setup.oplogs, send_terminal_index);

    // Replacing a durable `Start` with an unpaired entry breaks pairing
    let result = setup
        .debug_executor
        .playback(
            boundary,
            Some(vec![PlaybackOverride {
                index: send_start_index,
                oplog: PublicOplogEntry::NoOp(NoOpParams {
                    timestamp: Timestamp::now_utc(),
                }),
            }]),
        )
        .await;
    assert!(result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("pairing"),
        "expected a pairing validation error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );

    // A terminal entry must keep referencing the same `Start` index
    let result = setup
        .debug_executor
        .playback(
            boundary,
            Some(vec![PlaybackOverride {
                index: send_terminal_index,
                oplog: PublicOplogEntry::End(EndParams {
                    timestamp: Timestamp::now_utc(),
                    start_index: previous_index(send_start_index),
                    response: None,
                    forced_commit: false,
                }),
            }]),
        )
        .await;
    assert!(result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("pairing"),
        "expected a pairing validation error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );

    // Overrides beyond the recorded oplog are rejected
    let beyond_index =
        OplogIndex::from_u64(u64::from(setup.oplogs.last().unwrap().oplog_index) + 100);
    let result = setup
        .debug_executor
        .playback(
            boundary,
            Some(vec![PlaybackOverride {
                index: beyond_index,
                oplog: PublicOplogEntry::NoOp(NoOpParams {
                    timestamp: Timestamp::now_utc(),
                }),
            }]),
        )
        .await;
    assert!(result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("beyond the recorded oplog"),
        "expected a beyond-recorded-oplog validation error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );

    // A pairing-preserving override (same signature as the recorded entry) is accepted
    let recorded_start = setup.oplogs[u64::from(send_start_index) as usize - 1]
        .entry
        .clone();
    assert!(matches!(recorded_start, PublicOplogEntry::Start(_)));
    let playback_result = setup
        .debug_executor
        .playback(
            boundary,
            Some(vec![PlaybackOverride {
                index: send_start_index,
                oplog: recorded_start,
            }]),
        )
        .await?;
    assert_eq!(playback_result.current_index, boundary);

    setup.server.abort();
    Ok(())
}

// Playback overrides must preserve the full replay identity of a durable `Start` entry, not just
// its variant: replay claims `Start` entries by function name, durable function type, parent
// index, and request value (concurrent sibling calls are disambiguated by request), so changing
// any of these would make a replayed call claim the wrong record.
#[test]
#[tracing::instrument]
async fn test_playback_override_start_identity_validation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup =
        start_clock_debug_session(last_unique_id, deps, "playback-p3-override-identity").await?;

    let (send_start_index, send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);
    let boundary = invocation_boundary_after(&setup.oplogs, send_terminal_index);

    let recorded_send_start = setup
        .oplogs
        .iter()
        .find(|e| e.oplog_index == send_start_index)
        .expect("expected the recorded http::client::send Start entry")
        .entry
        .clone();

    // Replacing the `Start` with another recorded `Start` of a different host function keeps the
    // variant and pairing shape but changes the replay identity, so it must be rejected
    let other_start = setup
        .oplogs
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::Start(params) if params.function_name != "http::client::send" => {
                Some(e.entry.clone())
            }
            _ => None,
        })
        .expect("expected a recorded Start entry of a different host function");
    let result = setup
        .debug_executor
        .playback(
            boundary,
            Some(vec![PlaybackOverride {
                index: send_start_index,
                oplog: other_start,
            }]),
        )
        .await;
    assert!(result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("pairing"),
        "expected a pairing validation error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );

    // Dropping the request from the `Start` must be rejected
    let without_request = match recorded_send_start.clone() {
        PublicOplogEntry::Start(params) => PublicOplogEntry::Start(StartParams {
            request: None,
            ..params
        }),
        _ => panic!("expected a Start entry"),
    };
    let result = setup
        .debug_executor
        .playback(
            boundary,
            Some(vec![PlaybackOverride {
                index: send_start_index,
                oplog: without_request,
            }]),
        )
        .await;
    assert!(result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("pairing"),
        "expected a pairing validation error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );

    // Changing the request value (here: the requested uri path) must be rejected
    let mut mutated_json = serde_json::to_value(&recorded_send_start)?;
    mutate_json_strings(
        &mut mutated_json,
        "simulated-slow-request",
        "simulated-other-request",
    );
    assert_ne!(
        mutated_json,
        serde_json::to_value(&recorded_send_start)?,
        "expected the recorded Start entry to contain the request path"
    );
    let mutated_start: PublicOplogEntry = serde_json::from_value(mutated_json)?;
    let result = setup
        .debug_executor
        .playback(
            boundary,
            Some(vec![PlaybackOverride {
                index: send_start_index,
                oplog: mutated_start,
            }]),
        )
        .await;
    assert!(result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("request values differ"),
        "expected a request-value validation error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );

    setup.server.abort();
    Ok(())
}

// A raw rewind target inside an in-flight durable call must be rejected: replay could neither
// resolve the call from the entries before the target nor re-execute its side effects.
#[test]
#[tracing::instrument]
async fn test_rewind_target_inside_in_flight_durable_call_is_rejected(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup = start_clock_debug_session(last_unique_id, deps, "rewind-p3-in-flight").await?;

    let (send_start_index, send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);
    let boundary = invocation_boundary_after(&setup.oplogs, send_terminal_index);

    let playback_result = setup.debug_executor.playback(boundary, None).await?;
    assert_eq!(playback_result.current_index, boundary);

    let requests_before = setup.request_count.load(Ordering::SeqCst);

    // Any raw index after the `Start` but before its terminal entry lies inside the call
    let inside_call = previous_index(send_terminal_index);
    assert!(inside_call > send_start_index);
    let result = setup.debug_executor.rewind_raw(inside_call, false).await;
    assert!(result.is_err());
    assert!(
        last_jrpc_error_message(&setup.debug_executor).contains("in-flight durable call"),
        "expected an in-flight durable call validation error, got: {}",
        last_jrpc_error_message(&setup.debug_executor)
    );

    assert_eq!(
        setup.request_count.load(Ordering::SeqCst),
        requests_before,
        "a refused rewind must not re-execute the recorded HTTP call"
    );

    setup.server.abort();
    Ok(())
}

// Rewind must work after a playback that left the debug worker suspended and unloaded (a raw
// playback target stops the worker mid-invocation, so it unloads itself instead of idling in its
// invocation loop).
#[test]
#[tracing::instrument]
async fn test_rewind_after_playback_unloads_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut setup =
        start_clock_debug_session(last_unique_id, deps, "rewind-p3-after-unload").await?;

    let (send_start_index, send_terminal_index) = http_send_start_and_terminal(&setup.oplogs);
    let boundary = invocation_boundary_after(&setup.oplogs, send_terminal_index);
    let last_terminal_index = last_durable_terminal_before(&setup.oplogs, boundary);

    let first_boundary = nth_invocation_boundary(&setup.oplogs, 1);
    assert!(first_boundary < send_start_index);

    let requests_before = setup.request_count.load(Ordering::SeqCst);

    // Raw playback into the middle of the invocation: the worker suspends (and unloads) as soon
    // as it would run live past the target
    let playback_result = setup
        .debug_executor
        .playback_raw(last_terminal_index, None, false)
        .await?;
    assert_eq!(playback_result.current_index, last_terminal_index);

    // Give the suspended worker time to fully unload
    tokio::time::sleep(Duration::from_millis(500)).await;

    let rewind_result = setup.debug_executor.rewind(first_boundary).await?;
    assert_eq!(rewind_result.current_index, first_boundary);

    assert_eq!(
        setup.request_count.load(Ordering::SeqCst),
        requests_before,
        "debug playback and rewind must not re-execute the recorded HTTP call"
    );

    setup.server.abort();
    Ok(())
}

// Debug playback over an invocation that reads the environment through the P3-native
// `wasi:cli/environment@0.3` import: the replayed guest re-executes the (non-durable) P3
// environment host call inside the debug session, exercising the debug-mode P3 environment path.
#[test]
#[tracing::instrument]
async fn test_playback_with_p3_environment(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let component = regular_worker_executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env(
            "Environment",
            vec![("TEST_ENV".to_string(), "test-value".to_string())],
        )
        .store()
        .await?;

    let env_id = agent_id!("Environment", "debug-env-1");
    let agent_id = regular_worker_executor
        .start_agent(&component.id, env_id.clone())
        .await?;

    let result = regular_worker_executor
        .invoke_and_await_agent(&component, &env_id, "get_environment_p3", data_value!())
        .await?;
    let result_str = format!("{result:?}");
    assert!(
        result_str.contains("TEST_ENV"),
        "expected the enriched environment through the P3 import, got: {result_str}"
    );

    let oplogs = regular_worker_executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;
    let last_boundary = oplogs
        .iter()
        .rfind(|e| matches!(&e.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .map(|e| e.oplog_index)
        .expect("expected an AgentInvocationFinished entry");

    debug_executor.connect(&agent_id).await?;

    let playback_result = debug_executor.playback(last_boundary, None).await?;
    assert_eq!(playback_result.current_index, last_boundary);

    Ok(())
}

struct ClockDebugSetup {
    regular_worker_executor: TestWorkerExecutor,
    debug_executor: DebugWorkerExecutorClient,
    agent_id: AgentId,
    oplogs: Vec<PublicOplogEntryWithIndex>,
    request_count: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

/// Starts a regular and a debug executor, records one `Clock::sleep_during_request` invocation
/// (a P3 `http::client::send` raced against a P3 `monotonic_clock` sleep, producing concurrent
/// durable `Start`/terminal entry pairs in the oplog), then connects a debug session to it.
async fn start_clock_debug_session(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_name: &str,
) -> anyhow::Result<ClockDebugSetup> {
    let context = TestContext::new(last_unique_id);
    let regular_worker_executor = golem_worker_executor_test_utils::start(deps, &context).await?;
    let mut debug_executor = start_debug_worker_executor(&regular_worker_executor).await?;

    let (port, request_count, server) =
        counting_slow_request_server(Duration::from_millis(100)).await;

    let component = regular_worker_executor
        .component(
            &context.default_environment_id,
            "golem_it_host_api_tests_release",
        )
        .name("golem-it:host-api-tests")
        .with_env("Clock", vec![("PORT".to_string(), port.to_string())])
        .store()
        .await?;

    let clock_id = agent_id!("Clock", agent_name);
    let agent_id = regular_worker_executor
        .start_agent(&component.id, clock_id.clone())
        .await?;

    regular_worker_executor
        .invoke_and_await_agent(
            &component,
            &clock_id,
            "sleep_during_request",
            data_value!(30u64),
        )
        .await?;

    let oplogs = regular_worker_executor
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;

    debug_executor.connect(&agent_id).await?;

    Ok(ClockDebugSetup {
        regular_worker_executor,
        debug_executor,
        agent_id,
        oplogs,
        request_count,
        server,
    })
}

/// HTTP server for `/simulated-slow-request` that counts incoming requests, so tests can prove
/// that debug playback never re-executes the recorded HTTP call.
async fn counting_slow_request_server(
    delay: Duration,
) -> (u16, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let request_count = Arc::new(AtomicUsize::new(0));
    let counter = request_count.clone();

    let server = tokio::spawn(async move {
        let route = axum::Router::new().route(
            "/simulated-slow-request",
            axum::routing::get(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                    "slow response".to_string()
                }
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    (port, request_count, server)
}

/// Finds the recorded `http::client::send` durable call: its `Start` entry and the matching
/// terminal (`End` or `Cancelled`) entry.
fn http_send_start_and_terminal(oplogs: &[PublicOplogEntryWithIndex]) -> (OplogIndex, OplogIndex) {
    let start_index = oplogs
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::Start(params) if params.function_name == "http::client::send" => {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .expect("expected a durable http::client::send Start entry");
    let terminal_index = oplogs
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::End(params) if params.start_index == start_index => {
                Some(e.oplog_index)
            }
            PublicOplogEntry::Cancelled(params) if params.start_index == start_index => {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .expect("expected a terminal entry paired with the http::client::send Start");
    (start_index, terminal_index)
}

/// The `AgentInvocationFinished` boundary of the invocation containing the given index.
fn invocation_boundary_after(
    oplogs: &[PublicOplogEntryWithIndex],
    index: OplogIndex,
) -> OplogIndex {
    oplogs
        .iter()
        .find_map(|e| {
            (e.oplog_index > index
                && matches!(&e.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .then_some(e.oplog_index)
        })
        .expect("expected an AgentInvocationFinished entry after the given index")
}

/// The last durable terminal entry (`End` / `Cancelled`) before the given boundary — a raw
/// playback target at this index lies after all in-flight durable calls of the invocation.
fn last_durable_terminal_before(
    oplogs: &[PublicOplogEntryWithIndex],
    boundary: OplogIndex,
) -> OplogIndex {
    oplogs
        .iter()
        .rfind(|e| {
            e.oplog_index < boundary
                && matches!(
                    &e.entry,
                    PublicOplogEntry::End(_) | PublicOplogEntry::Cancelled(_)
                )
        })
        .map(|e| e.oplog_index)
        .expect("expected a durable terminal entry before the invocation boundary")
}

/// The message of the most recent JRPC error response received by the debug client.
fn last_jrpc_error_message(debug_executor: &DebugWorkerExecutorClient) -> String {
    debug_executor
        .all_read_messages()
        .iter()
        .rev()
        .find_map(|m| m.error.as_ref().map(|e| format!("{e:?}")))
        .unwrap_or_else(|| "no JRPC error message received".to_string())
}

fn nth_invocation_boundary(oplogs: &[PublicOplogEntryWithIndex], n: usize) -> OplogIndex {
    oplogs
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .nth(n - 1)
        .map(|entry| entry.oplog_index)
        .unwrap_or_else(|| panic!("No {n}th invocation boundary found"))
}

fn previous_index(index: OplogIndex) -> OplogIndex {
    OplogIndex::from_u64(u64::from(index) - 1)
}

/// Replaces `from` with `to` in every string of a JSON value, recursively — used to mutate the
/// recorded request value inside a serialized oplog entry without depending on its exact shape.
fn mutate_json_strings(value: &mut serde_json::Value, from: &str, to: &str) {
    match value {
        serde_json::Value::String(s) => {
            if s.contains(from) {
                *s = s.replace(from, to);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                mutate_json_strings(item, from, to);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                mutate_json_strings(item, from, to);
            }
        }
        _ => {}
    }
}

async fn run_repo_add_two(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    repo_id: &ParsedAgentId,
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
    repo_id: &ParsedAgentId,
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

    let agent_id =
        AgentId::from_agent_id(component.id, repo_id).map_err(|e| anyhow::anyhow!("{e}"))?;

    let oplogs = executor.get_oplog(&agent_id, OplogIndex::INITIAL).await?;

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
