// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `golem:agent/invocation-context` answers for the running invocation: its ID and its caller.
//! Exercises the `InvocationContextProbe` agent of the shared `agent_sdk_rust` component.

use crate::Tracing;
use crate::durability::assert_snapshot_recovery_loaded;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentUpdateMode;
use golem_common::model::caller::{ClaimVisibility, PublicCaller};
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, AgentStatus, InvocationId, OwnedAgentId, PromiseId};
use golem_common::schema::FromSchema;
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::callers;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides, start_with_snapshot_policy,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const PROBE: &str = "InvocationContextProbe";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Observation {
    invocation_id: Option<String>,
    original: String,
    immediate: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    constructor: Observation,
    constructor_caller: String,
    saved: Option<Observation>,
    loaded: Option<Observation>,
    method: Observation,
    method_caller: String,
}

#[derive(Debug, Deserialize)]
struct PeerReport {
    report: Report,
    triggered: String,
}

fn golem_user() -> (golem_common::model::caller::Caller, String) {
    let account_id = AccountId::new();
    (
        callers::account(account_id),
        format!(
            "account (\"{}\",\"account:{}\")",
            callers::GOLEM_ISSUER,
            account_id.0.hyphenated()
        ),
    )
}

/// What the probe prints for a caller: both roles and the number of tokens.
fn caller_text(original: &str, immediate: &str) -> String {
    format!("original={original} immediate={immediate} tokens=0")
}

/// The start of the text of an agent principal. The rest is the agent's fingerprint.
fn agent_text_prefix(agent_id: &AgentId) -> String {
    format!(
        "agent (\"{}\",\"agent:{}/",
        callers::GOLEM_ISSUER,
        golem_common::model::caller::agent_subject_uuid(agent_id).hyphenated()
    )
}

fn observation(invocation_id: Option<String>, caller: &str) -> Observation {
    Observation {
        invocation_id,
        original: caller.to_string(),
        immediate: caller.to_string(),
    }
}

async fn constructor_invocation_id(
    executor: &(impl TestDsl + Sync),
    agent_id: &OwnedAgentId,
) -> anyhow::Result<String> {
    let fingerprint = executor.get_worker_metadata(agent_id).await?.fingerprint;
    Ok(format!(
        "{}.{}",
        agent_id.agent_id,
        uuid::Uuid::new_v5(&fingerprint.0, b"constructor")
    ))
}

/// The start index and the text form of every `method` invocation recorded in the probe's
/// oplog, in order.
async fn method_invocations(
    executor: &impl TestDsl,
    agent_id: &OwnedAgentId,
    method: &str,
) -> anyhow::Result<Vec<(OplogIndex, String)>> {
    Ok(executor
        .get_oplog(agent_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter_map(|entry| match entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => match params.invocation {
                PublicAgentInvocation::AgentMethodInvocation(call)
                    if call.method_name == method =>
                {
                    Some((
                        entry.oplog_index,
                        format!("{}.{}", agent_id.agent_id, call.invocation_id),
                    ))
                }
                _ => None,
            },
            _ => None,
        })
        .collect())
}

/// The text form of every `report` invocation recorded in the probe's oplog, in order.
async fn report_invocation_ids(
    executor: &impl TestDsl,
    agent_id: &OwnedAgentId,
) -> anyhow::Result<Vec<String>> {
    Ok(method_invocations(executor, agent_id, "report")
        .await?
        .into_iter()
        .map(|(_, id)| id)
        .collect())
}

/// Waits until the probe's oplog records `count` started `report` invocations.
async fn await_report_invocation_ids(
    executor: &impl TestDsl,
    agent_id: &OwnedAgentId,
    count: usize,
) -> anyhow::Result<Vec<String>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let ids = report_invocation_ids(executor, agent_id).await?;
        if ids.len() >= count {
            return Ok(ids);
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "expected {count} report invocations, found {ids:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn invocation_context_answers_with_the_running_invocation_and_its_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let (creator, creator_text) = golem_user();
    let (user, user_text) = golem_user();

    let caller_name = format!("caller-{}", context.redis_prefix());
    let callee_name = format!("callee-{}", context.redis_prefix());
    let caller = agent_key!(PROBE, caller_name.clone());
    let callee = agent_key!(PROBE, callee_name.clone());
    let callee_id = executor.keyed_agent_id(component.environment_id, &callee)?;

    let first = executor
        .invoke_and_await_agent_as_caller(&component, &caller, creator, "report", data_value!())
        .await?;
    let caller_id = first.agent_id().clone();
    let report: Report = serde_json::from_str(&first.into_typed::<String>()?)?;
    let caller_reports = report_invocation_ids(&executor, &caller_id).await?;
    assert_eq!(caller_reports.len(), 1);
    let constructor_id = constructor_invocation_id(&executor, &caller_id).await?;
    assert_eq!(
        report.constructor,
        observation(Some(constructor_id.clone()), &creator_text),
        "the constructor sees its incarnation-bound invocation ID and its creator"
    );
    assert_eq!(
        report.method,
        observation(Some(caller_reports[0].clone()), &creator_text)
    );
    assert_eq!(report.saved, None);
    assert_eq!(report.loaded, None);

    let second: Report = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_caller(
                &component,
                &caller,
                user.clone(),
                "report",
                data_value!(),
            )
            .await?
            .into_typed::<String>()?,
    )?;
    let caller_reports = report_invocation_ids(&executor, &caller_id).await?;
    assert_eq!(caller_reports.len(), 2);
    assert_ne!(caller_reports[0], caller_reports[1]);
    assert_eq!(
        second.method,
        observation(Some(caller_reports[1].clone()), &user_text),
        "each invocation reads its own ID and its own caller"
    );
    assert_eq!(
        second.constructor,
        observation(Some(constructor_id.clone()), &creator_text),
        "the constructor observation keeps the creator"
    );
    assert_eq!(second.method_caller, caller_text(&user_text, &user_text));
    assert_eq!(
        second.constructor_caller,
        caller_text(&creator_text, &creator_text)
    );

    let peer: PeerReport = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_caller(
                &component,
                &caller,
                user.clone(),
                "report_of_peer",
                data_value!(callee_name),
            )
            .await?
            .into_typed::<String>()?,
    )?;
    let peer_report = peer.report;
    let callee_reports = await_report_invocation_ids(&executor, &callee_id, 2).await?;
    assert_eq!(callee_reports.len(), 2);
    assert_eq!(
        peer.triggered, callee_reports[1],
        "the caller's invocation metadata names the callee's invocation"
    );
    // The peer keeps the user as the original caller and sees the calling agent as the immediate
    // caller, in its constructor (create on demand) and in its method.
    let agent_prefix = agent_text_prefix(&caller_id.agent_id);
    for observed in [&peer_report.constructor, &peer_report.method] {
        assert_eq!(observed.original, user_text);
        assert!(
            observed.immediate.starts_with(&agent_prefix),
            "{} is not {agent_prefix}",
            observed.immediate
        );
    }
    assert_eq!(
        peer_report.constructor.invocation_id,
        Some(constructor_invocation_id(&executor, &callee_id).await?)
    );
    assert_eq!(
        peer_report.method.invocation_id,
        Some(callee_reports[0].clone())
    );
    assert!(
        peer_report
            .method_caller
            .starts_with(&format!("original={user_text} immediate={agent_prefix}"))
            && peer_report.method_caller.ends_with(" tokens=0"),
        "{}",
        peer_report.method_caller
    );

    // The peer recorded the same caller for its creation, the awaited call and the triggered
    // call: the user, and the calling agent as it is.
    let calling_agent = callers::agent_in(
        caller_id.agent_id,
        caller.agent_type.clone(),
        executor.get_worker_metadata(&caller_id).await?.fingerprint,
        component.environment_id,
    )
    .immediate;
    let expected = golem_common::model::caller::Caller {
        immediate: calling_agent,
        ..user
    };
    let recorded = executor.recorded_callers(&callee_id.agent_id).await;
    let count = |wanted: &str| recorded.iter().filter(|(name, _)| name == wanted).count();
    assert_eq!(count("create"), 1, "{recorded:?}");
    assert_eq!(count("report"), 2, "{recorded:?}");
    for (name, recorded) in recorded {
        assert_eq!(recorded, expected, "{name}");
    }

    let old_id = constructor_id
        .parse::<golem_common::model::OwnedInvocationId>()
        .map_err(anyhow::Error::msg)?;
    executor.delete_worker(&caller_id).await?;
    let (new_creator, new_creator_text) = golem_user();
    let recreated = executor
        .invoke_and_await_agent_as_caller(&component, &caller, new_creator, "report", data_value!())
        .await?;
    assert_eq!(recreated.agent_id(), &caller_id);
    let recreated: Report = serde_json::from_str(&recreated.into_typed::<String>()?)?;
    let new_constructor_id = constructor_invocation_id(&executor, &caller_id).await?;
    assert_ne!(constructor_id, new_constructor_id);
    assert_eq!(
        recreated.constructor,
        observation(Some(new_constructor_id.clone()), &new_creator_text)
    );
    assert!(
        executor
            .get_invocation(&caller_id.agent_id, caller_id.environment_id, old_id.id())
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .is_none(),
        "the deleted incarnation's constructor must not resolve on its successor"
    );

    drop(executor);
    let executor = start(deps, &context).await?;
    assert!(!executor.worker_is_cached(&caller_id).await);
    let new_id = new_constructor_id
        .parse::<golem_common::model::OwnedInvocationId>()
        .map_err(anyhow::Error::msg)?;
    let new_constructor = executor
        .get_invocation(&caller_id.agent_id, caller_id.environment_id, new_id.id())
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .expect("the successor constructor is readable cold");
    assert_eq!(
        new_constructor.kind,
        golem_api_grpc::proto::golem::workerexecutor::v1::InvocationKind::Constructor as i32
    );
    assert_eq!(
        new_constructor.state,
        golem_api_grpc::proto::golem::workerexecutor::v1::InvocationLifecycleState::Completed
            as i32
    );
    assert!(
        executor
            .get_invocation(&caller_id.agent_id, caller_id.environment_id, old_id.id())
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?
            .is_none()
    );
    assert!(
        !executor.worker_is_loaded(&caller_id).await,
        "cold invocation reads do not activate the guest"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn snapshot_save_and_load_see_no_invocation_and_the_agent_itself(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let (creator, creator_text) = golem_user();
    let (user, user_text) = golem_user();
    let probe = agent_key!(PROBE, format!("snapshot-{}", context.redis_prefix()));
    let mut probe_id = None;
    for round in 0..3 {
        let result = executor
            .invoke_and_await_agent_as_caller(
                &component,
                &probe,
                if round == 0 {
                    creator.clone()
                } else {
                    user.clone()
                },
                "report",
                data_value!(),
            )
            .await?;
        probe_id = Some(result.agent_id().clone());
    }
    let probe_id = probe_id.expect("the probe was invoked");
    let oplog = executor.get_oplog(&probe_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_))),
        "the snapshot policy should have saved a snapshot"
    );

    drop(executor);
    let executor = start_with_snapshot_policy(deps, &context, policy).await?;
    let mut events = executor.capture_output(&probe_id).await?;
    executor.resume(&probe_id, false).await?;
    executor
        .wait_for_status(&probe_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    let report: Report = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_caller(&component, &probe, user, "report", data_value!())
            .await?
            .into_typed::<String>()?,
    )?;
    assert_snapshot_recovery_loaded(&mut events).await;

    let itself = agent_text_prefix(&probe_id.agent_id);
    assert_eq!(
        report.constructor,
        observation(
            Some(constructor_invocation_id(&executor, &probe_id).await?),
            &creator_text
        )
    );
    assert_eq!(
        report.constructor_caller,
        caller_text(&creator_text, &creator_text)
    );
    assert_eq!(report.method_caller, caller_text(&user_text, &user_text));
    for observed in [&report.saved, &report.loaded] {
        let observed = observed
            .as_ref()
            .expect("the snapshot callbacks are observed");
        assert_eq!(observed.invocation_id, None);
        assert!(
            observed.original.starts_with(&itself),
            "{}",
            observed.original
        );
        assert!(
            observed.immediate.starts_with(&itself),
            "{}",
            observed.immediate
        );
    }
    let reports = report_invocation_ids(&executor, &probe_id).await?;
    assert_eq!(
        report.method,
        observation(reports.last().cloned(), &user_text)
    );

    drop(executor);
    Ok(())
}

/// A manual update saves a snapshot on the old revision and loads it on the new one. Both run
/// outside any invocation, with the agent itself in both roles.
#[test]
#[timeout("180s")]
#[tracing::instrument]
async fn manual_update_save_and_load_see_no_invocation_and_the_agent_itself(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let (user, user_text) = golem_user();
    let probe = agent_key!(PROBE, format!("manual-update-{}", context.redis_prefix()));
    let probe_id = executor.start_agent(&component.id, probe.clone()).await?;

    let updated = executor
        .update_component(&component.id, "golem_it_agent_sdk_rust_release")
        .await?;
    executor
        .manual_update_worker(&probe_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&probe_id, updated.revision, Duration::from_secs(30))
        .await?;

    let report: Report = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_caller(&component, &probe, user, "report", data_value!())
            .await?
            .into_typed::<String>()?,
    )?;

    let itself = agent_text_prefix(&probe_id.agent_id);
    for observed in [&report.saved, &report.loaded] {
        let observed = observed
            .as_ref()
            .expect("the update runs both snapshot callbacks");
        assert_eq!(observed.invocation_id, None);
        assert!(
            observed.original.starts_with(&itself),
            "{}",
            observed.original
        );
        assert!(
            observed.immediate.starts_with(&itself),
            "{}",
            observed.immediate
        );
    }
    assert_eq!(report.method_caller, caller_text(&user_text, &user_text));

    drop(executor);
    Ok(())
}

/// A manual update runs its snapshot save and load as the agent itself, with no invocation. The
/// account that requested the update is recorded in the update's entries, never as the caller of
/// either snapshot call.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn manual_update_snapshots_run_as_the_agent_and_record_the_requester(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let (user, _) = golem_user();
    let (requester, requester_text) = golem_user();
    let probe = agent_key!(PROBE, format!("update-{}", context.redis_prefix()));
    let probe_id = executor
        .invoke_and_await_agent_as_caller(&component, &probe, user.clone(), "report", data_value!())
        .await?
        .agent_id()
        .clone();
    // The next revision is the same probe, so its load hook reads what the save hook wrote.
    let target = executor
        .update_component(&component.id, "golem_it_agent_sdk_rust_release")
        .await?;
    let attempt = executor
        .update_worker_as_caller(
            &probe_id,
            AgentUpdateMode::Manual,
            target.revision,
            false,
            requester.clone(),
        )
        .await?;
    executor
        .wait_for_component_revision(&probe_id, target.revision, Duration::from_secs(30))
        .await?;

    let report: Report = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_caller(&component, &probe, user, "report", data_value!())
            .await?
            .into_typed::<String>()?,
    )?;
    let itself = agent_text_prefix(&probe_id.agent_id);
    for observed in [&report.saved, &report.loaded] {
        let observed = observed
            .as_ref()
            .expect("the update runs the snapshot callbacks");
        assert_eq!(observed.invocation_id, None);
        for role in [&observed.original, &observed.immediate] {
            assert!(role.starts_with(&itself), "{role}");
            assert_ne!(role, &requester_text);
        }
    }

    let expected = PublicCaller::new(&requester, ClaimVisibility::Hidden);
    let requesters = executor
        .get_oplog(&probe_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter_map(|entry| match entry.entry {
            PublicOplogEntry::PendingUpdate(params) => {
                Some((params.update_attempt_index, params.requester))
            }
            PublicOplogEntry::PendingAgentInvocation(params) => match params.invocation {
                PublicAgentInvocation::ManualUpdate(manual) => {
                    Some((entry.oplog_index, manual.requester))
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        requesters,
        vec![(attempt, expected.clone()), (attempt, expected)]
    );

    drop(executor);
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn executor_refuses_an_invocation_with_no_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::InvocationRequestKey;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let probe = agent_key!(PROBE, format!("no-caller-{}", context.redis_prefix()));
    let probe_id = executor.start_agent(&component.id, probe.clone()).await?;
    let oplog_before = executor.oplog_max_index(&probe_id).await?;

    let method_parameters: golem_schema::proto::golem::schema::SchemaValue =
        golem_common::schema::SchemaValue::Record { fields: vec![] }
            .try_into()
            .map_err(anyhow::Error::msg)?;
    let error = executor
        .invoke_agent_session({
            let mut start = executor
                .invocation(&component, "report")
                .on_agent(&probe_id)
                .build()?;
            start.idempotency_key = None;
            start.input = Some(method_parameters);
            start.caller = None;
            start.request_key = Some(InvocationRequestKey::external(None, None).into());
            start
        })
        .await
        .expect_err("an invocation without a caller must be refused");
    assert!(
        error.to_string().contains("caller is missing"),
        "unexpected refusal: {error}"
    );

    let oplog = executor.get_oplog(&probe_id, oplog_before).await?;
    assert!(
        oplog
            .iter()
            .filter(|entry| entry.oplog_index > oplog_before)
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))),
        "the refused invocation must not write an AgentInvocationStarted entry"
    );

    drop(executor);
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn slot_fork_refuses_a_missing_caller(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        ForkStreamSlotRequest, fork_stream_slot_response,
    };

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let source = agent_key!(PROBE, format!("fork-source-{}", context.redis_prefix()));
    let source_id = executor.start_agent(&component.id, source).await?;
    let target_id = crate::fork::fresh_fork_target(&source_id);

    let response = executor
        .client
        .clone()
        .fork_stream_slot(ForkStreamSlotRequest {
            source_agent_id: Some(source_id.agent_id.into()),
            target_agent_id: Some(target_id.agent_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(golem_service_base::model::auth::AuthCtx::System.into()),
            caller: None,
            ..Default::default()
        })
        .await?
        .into_inner();
    let Some(fork_stream_slot_response::Result::Failure(failure)) = response.result else {
        panic!("a slot fork without a caller must fail, got {response:?}");
    };
    assert!(
        format!("{failure:?}").contains("caller is missing"),
        "unexpected failure: {failure:?}"
    );

    drop(executor);
    Ok(())
}

/// The index and the anchor of each recorded `generate_idempotency-key` call, in order.
fn idempotency_key_starts(
    oplog: &[PublicOplogEntryWithIndex],
) -> Vec<(OplogIndex, Option<OplogIndex>)> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::api::generate_idempotency-key" =>
            {
                Some((entry.oplog_index, params.invocation_id.map(|id| id.anchor)))
            }
            _ => None,
        })
        .collect()
}

/// The invocation ID of the method invocation whose `AgentInvocationStarted` is at `start`.
fn invocation_id_at(
    oplog: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> Option<InvocationId> {
    oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::AgentInvocationStarted(params) if entry.oplog_index == start => {
            match &params.invocation {
                PublicAgentInvocation::AgentMethodInvocation(call) => Some(call.invocation_id),
                _ => None,
            }
        }
        _ => None,
    })
}

/// The index of the `AgentInvocationFinished` of the method invocation whose
/// `AgentInvocationStarted` is at `start`.
fn finished_of(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Option<OplogIndex> {
    let invocation_id = invocation_id_at(oplog, start)?;
    oplog
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(params)
                    if params.invocation_id == invocation_id
            )
        })
        .map(|entry| entry.oplog_index)
}

/// The index of the `AgentInvocationSettled` of the method invocation whose
/// `AgentInvocationStarted` is at `start`.
fn settled_of(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Option<OplogIndex> {
    let invocation_id = invocation_id_at(oplog, start)?;
    oplog
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationSettled(params)
                    if params.invocation_id == invocation_id
            )
        })
        .map(|entry| entry.oplog_index)
}

/// Invokes `method` on a fresh probe twice, as two different callers. Each invocation spawns
/// a guest future that makes a durable call and observes the invocation context; `reports`
/// returns the observations, and so does `method` itself when `returns_observation` is set.
/// Checks that each future reads the ID and the caller of the method it ran in, that its
/// durable call is numbered under that invocation and recorded before the background work of the
/// invocation settles, and after the invocation's result when the future observes after the
/// method returned (`returns_observation` unset), and that a real restart replays the same
/// observations and calls.
async fn check_spawned_futures_read_their_method_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    agent_sdk_rust: &PrecompiledComponent,
    method: &str,
    reports: &str,
    returns_observation: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let (creator, creator_text) = golem_user();
    let (user, user_text) = golem_user();
    let probe = agent_key!(PROBE, format!("{method}-{}", context.redis_prefix()));
    let probe_id = executor.start_agent(&component.id, probe.clone()).await?;

    let mut returned = Vec::new();
    for caller in [creator, user] {
        let result = executor
            .invoke_and_await_agent_as_caller(&component, &probe, caller, method, data_value!())
            .await?;
        if returns_observation {
            returned.push(serde_json::from_str::<Observation>(
                &result.into_typed::<String>()?,
            )?);
        }
    }
    let live: Vec<Observation> = serde_json::from_str(
        &executor
            .invoke_and_await_agent(&component, &probe, reports, data_value!())
            .await?
            .into_typed::<String>()?,
    )?;
    if returns_observation {
        assert_eq!(
            returned, live,
            "the method returns the observation it keeps"
        );
    }
    let invocations = method_invocations(&executor, &probe_id, method).await?;
    assert_eq!(invocations.len(), 2);
    assert_eq!(
        live,
        vec![
            observation(Some(invocations[0].1.clone()), &creator_text),
            observation(Some(invocations[1].1.clone()), &user_text),
        ],
        "a spawned future reads the ID and the caller of the method it ran in"
    );
    let oplog = executor.get_oplog(&probe_id, OplogIndex::INITIAL).await?;
    let starts = idempotency_key_starts(&oplog);
    assert_eq!(
        starts.iter().map(|(_, anchor)| *anchor).collect::<Vec<_>>(),
        vec![Some(invocations[0].0), Some(invocations[1].0)],
        "the durable calls of a spawned future are numbered under the method's invocation"
    );
    for ((start, _), (invocation, _)) in starts.iter().zip(&invocations) {
        let settled = settled_of(&oplog, *invocation)
            .expect("the background work of each invocation of the method should settle");
        assert!(
            start < &settled,
            "the durable call at {start} should be recorded before its invocation settles at {settled}"
        );
        if !returns_observation {
            let finished = finished_of(&oplog, *invocation)
                .expect("each invocation of the method should record its result");
            assert!(
                start > &finished,
                "the durable call at {start} should be recorded after the result at {finished}"
            );
        }
    }
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_))),
        "recovery must replay the invocations, not load a snapshot"
    );

    drop(executor);
    let executor = start(deps, &context).await?;
    let replayed: Vec<Observation> = serde_json::from_str(
        &executor
            .invoke_and_await_agent(&component, &probe, reports, data_value!())
            .await?
            .into_typed::<String>()?,
    )?;
    assert_eq!(
        replayed, live,
        "replay reads the same IDs and callers as the live run"
    );
    let oplog = executor.get_oplog(&probe_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        idempotency_key_starts(&oplog),
        starts,
        "replay claims the recorded calls instead of making new ones"
    );

    drop(executor);
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn a_spawned_guest_future_reads_its_method_invocation_live_and_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_spawned_futures_read_their_method_invocation(
        last_unique_id,
        deps,
        agent_sdk_rust,
        "report_from_spawned_future",
        "spawned_future_reports",
        true,
    )
    .await
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn background_work_after_the_method_returns_reads_its_invocation_live_and_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_spawned_futures_read_their_method_invocation(
        last_unique_id,
        deps,
        agent_sdk_rust,
        "observe_after_return",
        "after_return_reports",
        false,
    )
    .await
}

/// A promise completes while an invocation waits for it, after the executor unloaded the agent.
/// The resumed invocation still has the caller of the request that started it: Golem wakes the
/// agent itself, and the request that completes the promise has another caller.
#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn a_promise_wake_up_keeps_the_caller_of_the_waiting_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    // The executor checks a blocked guest for suspension at this interval. The default interval
    // equals the wait for the suspended status below.
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_rust)
        .store()
        .await?;
    let (waiter, waiter_text) = golem_user();
    let probe = agent_key!(PROBE, format!("promise-{}", context.redis_prefix()));
    let agent_id = executor.start_agent(&component.id, probe.clone()).await?;

    let promise = executor
        .invoke_and_await_agent(&component, &probe, "create_promise", data_value!())
        .await?
        .into_return_value()
        .expect("create_promise returns a promise ID");
    let waiting = tokio::spawn({
        let (executor, component, probe) = (executor.clone(), component.clone(), probe.clone());
        let params = crate::raw_params(vec![promise.clone()]);
        async move {
            executor
                .invoke_and_await_agent_as_caller(
                    &component,
                    &probe,
                    waiter,
                    "caller_around_promise",
                    params,
                )
                .await
        }
        .in_current_span()
    });
    executor
        .wait_for_status(&agent_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_is_loaded(&agent_id).await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?;

    executor
        .complete_promise(
            &context.default_environment_id,
            &PromiseId::from_value(&promise).expect("the guest returned a promise ID"),
            vec![1],
        )
        .await?;

    let observed: Vec<String> = waiting.await??.into_typed()?;
    let expected = caller_text(&waiter_text, &waiter_text);
    assert_eq!(
        observed,
        vec![expected.clone(), expected],
        "the caller before the wait and after the wake-up is the caller of the waiting request"
    );
    Ok(())
}
