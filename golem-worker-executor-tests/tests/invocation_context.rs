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
use golem_common::model::agent::{GolemUserPrincipal, ParsedAgentKey, Principal};
use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{AgentId, AgentStatus};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::SnapshotPolicy;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
    start_with_snapshot_policy,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

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
    constructor_principal: String,
    saved: Option<Observation>,
    loaded: Option<Observation>,
    method: Observation,
    method_principal: String,
}

#[derive(Debug, Deserialize)]
struct PeerReport {
    report: Report,
    triggered: String,
}

fn golem_user() -> (Principal, String) {
    let account_id = AccountId::new();
    (
        Principal::GolemUser(GolemUserPrincipal { account_id }),
        format!("golem-user:{account_id}"),
    )
}

fn observation(invocation_id: Option<String>, caller: &str) -> Observation {
    Observation {
        invocation_id,
        original: caller.to_string(),
        immediate: caller.to_string(),
    }
}

/// The text form of every `report` invocation recorded in the probe's oplog, in order.
async fn report_invocation_ids(
    executor: &impl TestDsl,
    agent_id: &AgentId,
    parsed: &ParsedAgentKey,
) -> anyhow::Result<Vec<String>> {
    Ok(executor
        .get_oplog(agent_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter_map(|entry| match entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params) => match params.invocation {
                PublicAgentInvocation::AgentMethodInvocation(call)
                    if call.method_name == "report" =>
                {
                    Some(format!("{parsed}.{}", call.invocation_id))
                }
                _ => None,
            },
            _ => None,
        })
        .collect())
}

/// Waits until the probe's oplog records `count` started `report` invocations.
async fn await_report_invocation_ids(
    executor: &impl TestDsl,
    agent_id: &AgentId,
    parsed: &ParsedAgentKey,
    count: usize,
) -> anyhow::Result<Vec<String>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let ids = report_invocation_ids(executor, agent_id, parsed).await?;
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
    let caller = agent_id!(PROBE, caller_name.clone());
    let callee = agent_id!(PROBE, callee_name.clone());
    let caller_id = AgentId {
        component_id: component.id,
        agent_id: caller.to_string(),
    };
    let callee_id = AgentId {
        component_id: component.id,
        agent_id: callee.to_string(),
    };

    let report: Report = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_principal(
                &component,
                &caller,
                creator,
                "report",
                data_value!(),
            )
            .await?
            .into_typed::<String>()?,
    )?;
    let caller_reports = report_invocation_ids(&executor, &caller_id, &caller).await?;
    assert_eq!(caller_reports.len(), 1);
    assert_eq!(
        report.constructor,
        observation(None, &creator_text),
        "the constructor is not an invocation and sees its creator"
    );
    assert_eq!(
        report.method,
        observation(Some(caller_reports[0].clone()), &creator_text)
    );
    assert_eq!(report.saved, None);
    assert_eq!(report.loaded, None);

    let second: Report = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_principal(
                &component,
                &caller,
                user.clone(),
                "report",
                data_value!(),
            )
            .await?
            .into_typed::<String>()?,
    )?;
    let caller_reports = report_invocation_ids(&executor, &caller_id, &caller).await?;
    assert_eq!(caller_reports.len(), 2);
    assert_ne!(caller_reports[0], caller_reports[1]);
    assert_eq!(
        second.method,
        observation(Some(caller_reports[1].clone()), &user_text),
        "each invocation reads its own ID and its own caller"
    );
    assert_eq!(
        second.constructor,
        observation(None, &creator_text),
        "the constructor observation keeps the creator"
    );
    assert_eq!(second.method_principal, user_text);
    assert_eq!(second.constructor_principal, creator_text);

    let peer: PeerReport = serde_json::from_str(
        &executor
            .invoke_and_await_agent_as_principal(
                &component,
                &caller,
                user,
                "report_of_peer",
                data_value!(callee_name),
            )
            .await?
            .into_typed::<String>()?,
    )?;
    let peer_report = peer.report;
    let callee_reports = await_report_invocation_ids(&executor, &callee_id, &callee, 2).await?;
    assert_eq!(callee_reports.len(), 2);
    assert_eq!(
        peer.triggered, callee_reports[1],
        "the caller's invocation metadata names the callee's invocation"
    );
    // Until the original caller propagates through RPC, `original` falls back to the immediate
    // caller.
    let caller_principal = format!("agent:{caller}");
    assert_eq!(
        peer_report.constructor,
        observation(None, &caller_principal),
        "an agent created by RPC sees the calling agent as its creator"
    );
    assert_eq!(
        peer_report.method,
        observation(Some(callee_reports[0].clone()), &caller_principal)
    );
    assert_eq!(peer_report.method_principal, caller_principal);

    drop(executor);
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
    let probe = agent_id!(PROBE, format!("snapshot-{}", context.redis_prefix()));
    let probe_id = AgentId {
        component_id: component.id,
        agent_id: probe.to_string(),
    };

    for round in 0..3 {
        executor
            .invoke_and_await_agent_as_principal(
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
    }
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
            .invoke_and_await_agent_as_principal(&component, &probe, user, "report", data_value!())
            .await?
            .into_typed::<String>()?,
    )?;
    assert_snapshot_recovery_loaded(&mut events).await;

    let itself = format!("agent:{probe}");
    assert_eq!(report.constructor, observation(None, &creator_text));
    assert_eq!(report.constructor_principal, creator_text);
    assert_eq!(report.method_principal, user_text);
    assert_eq!(report.saved, Some(observation(None, &itself)));
    assert_eq!(report.loaded, Some(observation(None, &itself)));
    let reports = report_invocation_ids(&executor, &probe_id, &probe).await?;
    assert_eq!(
        report.method,
        observation(reports.last().cloned(), &user_text)
    );

    drop(executor);
    Ok(())
}
