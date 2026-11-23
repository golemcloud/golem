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

// The executor's invocation reads: `GetInvocation`, `ListInvocations` and `LookupInvocations`.

use crate::Tracing;
use chrono::Utc;
use golem_api_grpc::proto::golem::worker::Cursor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationKind, InvocationLifecycleState, InvocationSummary, ListInvocationsRequest,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{AgentInvocation, IdempotencyKey, InvocationId};
use golem_common::{agent_key, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, callers, start,
};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);

fn id_of(summary: &InvocationSummary) -> InvocationId {
    summary.invocation_id.unwrap().into()
}

fn state_of(summary: &InvocationSummary) -> InvocationLifecycleState {
    InvocationLifecycleState::try_from(summary.state).unwrap()
}

/// Get and list name each invocation with its state, kind and operation; pages of two list
/// every invocation once, newest change first; cursors and filters are checked.
#[test]
#[timeout("4m")]
async fn invocations_are_read_by_id_and_listed_in_pages(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "invocation-reads");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let owner = executor
        .start_agent(
            &component.id,
            agent_key!("ScheduledInvocationClient", "invocation-reads-owner"),
        )
        .await?;
    let env = component.environment_id;
    let invocation = |key: &InvocationId| AgentInvocation::AgentMethod {
        idempotency_key: *key,
        method_name: "inc_global_by".to_string(),
        input: data_value!(1u64).into_parts().1.try_into().unwrap(),
        allow_streams: false,
        invocation_context: InvocationContextStack::fresh(),
        caller: callers::agent(
            owner.agent_id,
            AgentTypeName("ScheduledInvocationClient".to_string()),
        ),
        scope_card: None,
    };
    executor
        .invoke_and_await_agent(&component, &server, "get_global_value", data_value!())
        .await?;
    let owned = target.clone();
    let worker = executor.active_agent(&owned).await.unwrap().primary();
    let due = Utc::now() + chrono::Duration::hours(1);

    let cancelled = InvocationId(uuid::Uuid::new_v4());
    let receipt = worker
        .schedule_invocation(invocation(&cancelled), due)
        .await?;
    assert!(
        worker
            .cancel_scheduled_invocation(
                cancelled,
                receipt,
                &callers::agent_principal(
                    owner.agent_id,
                    AgentTypeName("ScheduledInvocationClient".to_string())
                )
            )
            .await?
    );
    let scheduled = InvocationId(uuid::Uuid::new_v4());
    worker
        .schedule_invocation(invocation(&scheduled), due)
        .await?;
    drop(worker);

    // Get names the state, kind and operation; an unknown ID is no invocation, not an error.
    let got = executor
        .get_invocation(&target.agent_id, env, scheduled)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .expect("the scheduled invocation");
    assert_eq!(state_of(&got), InvocationLifecycleState::Scheduled);
    assert_eq!(got.kind, InvocationKind::Method as i32);
    assert_eq!(got.operation.as_deref(), Some("inc_global_by"));
    let cancelled_got = executor
        .get_invocation(&target.agent_id, env, cancelled)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .expect("the cancelled invocation");
    assert_eq!(
        state_of(&cancelled_got),
        InvocationLifecycleState::Cancelled
    );
    assert_eq!(
        executor
            .get_invocation(&target.agent_id, env, InvocationId(uuid::Uuid::new_v4()))
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        None
    );

    // The whole list: the constructor run, the awaited call, the cancelled and the scheduled one.
    let all = executor
        .list_invocations(
            &target.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 100,
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(all.next_cursor, None);
    let ids: Vec<InvocationId> = all.invocations.iter().map(id_of).collect();
    assert_eq!(ids.len(), 4, "{:?}", all.invocations);
    assert_eq!(ids[0], scheduled);
    assert_eq!(ids[1], cancelled);
    assert!(
        all.invocations
            .iter()
            .any(|summary| summary.kind == InvocationKind::Constructor as i32
                && state_of(summary) == InvocationLifecycleState::Completed
                && summary.operation.is_none())
    );

    // Pages of two give the same invocations in the same order, each once.
    let mut paged = Vec::new();
    let mut cursor: Option<Cursor> = None;
    loop {
        let page = executor
            .list_invocations(
                &target.agent_id,
                env,
                ListInvocationsRequest {
                    page_size: 2,
                    cursor: cursor.clone(),
                    ..Default::default()
                },
            )
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert!(page.invocations.len() <= 2);
        paged.extend(page.invocations.iter().map(id_of));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(paged, ids);

    // Filters by state and kind.
    let methods_in_flight = executor
        .list_invocations(
            &target.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 100,
                states: vec![
                    InvocationLifecycleState::Scheduled as i32,
                    InvocationLifecycleState::Cancelled as i32,
                ],
                kind: Some(InvocationKind::Method as i32),
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(
        methods_in_flight
            .invocations
            .iter()
            .map(id_of)
            .collect::<Vec<_>>(),
        vec![scheduled, cancelled]
    );

    // A cursor of another agent, and a malformed one, are invalid requests.
    let first_page = executor
        .list_invocations(
            &target.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 1,
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let foreign = first_page.next_cursor.expect("more than one invocation");
    for bad in [
        foreign,
        Cursor {
            value: "not-a-cursor".to_string(),
        },
    ] {
        let refused = executor
            .list_invocations(
                &owner.agent_id,
                env,
                ListInvocationsRequest {
                    page_size: 1,
                    cursor: Some(bad.clone()),
                    ..Default::default()
                },
            )
            .await?;
        let error = refused.expect_err("an invalid cursor is refused");
        assert!(
            format!("{error:?}").contains("cursor"),
            "{bad:?}: {error:?}"
        );
    }

    // A deleted agent has no invocations to read.
    executor.delete_worker(&target).await?;
    let gone = executor
        .get_invocation(&target.agent_id, env, scheduled)
        .await?;
    assert!(format!("{gone:?}").contains("AgentNotFound"), "{gone:?}");
    Ok(())
}

/// Lookup finds the caller's keyed invocations on every operation, narrows to one operation,
/// and finds nothing for another caller or a caller without a scope.
#[test]
#[timeout("4m")]
async fn invocations_are_looked_up_by_the_callers_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let server = agent_key!("ScheduledInvocationServer", "invocation-lookup");
    let target = executor.start_agent(&component.id, server.clone()).await?;
    let env = component.environment_id;

    let key = IdempotencyKey::fresh();
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &server,
            &key,
            "inc_global_by",
            data_value!(3u64),
        )
        .await?;
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &server,
            &key,
            "get_global_value",
            data_value!(),
        )
        .await?;

    let operations = |found: &[InvocationSummary]| {
        let mut operations: Vec<String> = found
            .iter()
            .map(|summary| {
                assert_eq!(state_of(summary), InvocationLifecycleState::Completed);
                summary.operation.clone().unwrap()
            })
            .collect();
        operations.sort();
        operations
    };
    let found = executor
        .lookup_invocations(&target.agent_id, env, &key, None)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(
        operations(&found),
        vec!["get_global_value", "inc_global_by"]
    );
    let narrowed = executor
        .lookup_invocations(&target.agent_id, env, &key, Some("inc_global_by"))
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(operations(&narrowed), vec!["inc_global_by"]);
    // Each found invocation is the one get reads by its ID.
    let by_id = executor
        .get_invocation(&target.agent_id, env, id_of(&narrowed[0]))
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(by_id.as_ref(), Some(&narrowed[0]));
    // A key nobody used finds nothing.
    assert_eq!(
        executor
            .lookup_invocations(&target.agent_id, env, &IdempotencyKey::fresh(), None)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        Vec::new()
    );

    let AuthCtx::User(user) = executor.auth_ctx() else {
        anyhow::bail!("the test executor authenticates as a user");
    };
    let mut other = user.clone();
    other.account_id = AccountId::new();
    for caller in [AuthCtx::User(other), AuthCtx::System] {
        let found = executor
            .as_caller(caller.clone())
            .lookup_invocations(&target.agent_id, env, &key, None)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert_eq!(found, Vec::new(), "{caller:?}");
    }

    // After a delete and re-create the key finds nothing: the new incarnation derives other IDs.
    executor.delete_worker(&target).await?;
    executor.start_agent(&component.id, server.clone()).await?;
    assert_eq!(
        executor
            .lookup_invocations(&target.agent_id, env, &key, None)
            .await?
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        Vec::new()
    );
    Ok(())
}

/// An ephemeral agent's finished invocation is listed after its instance is gone: the agent is
/// reconstructed from its oplog, and its few results stay in the exact recent results.
#[test]
#[timeout("4m")]
async fn a_finished_ephemeral_invocation_is_listed_after_its_instance_is_gone(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let ephemeral = agent_key!("ScheduledEphemeralTarget", "invocation-reads-ephemeral");
    let key = IdempotencyKey::fresh();
    let result = executor
        .invoke_and_await_agent_with_key(&component, &ephemeral, &key, "inc", data_value!(0u64))
        .await?;
    let agent = result.agent_id().clone();
    let env = component.environment_id;

    let listed = executor
        .list_invocations(
            &agent.agent_id,
            env,
            ListInvocationsRequest {
                page_size: 100,
                ..Default::default()
            },
        )
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(listed.invocations.len(), 2, "{:?}", listed.invocations);
    let constructor = &listed.invocations[1];
    assert_eq!(constructor.kind, InvocationKind::Constructor as i32);
    assert_eq!(state_of(constructor), InvocationLifecycleState::Completed);
    let inc = &listed.invocations[0];
    assert_eq!(state_of(inc), InvocationLifecycleState::Completed);
    assert_eq!(inc.operation.as_deref(), Some("inc"));
    let found = executor
        .lookup_invocations(&agent.agent_id, env, &key, None)
        .await?
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(found, vec![inc.clone()]);
    Ok(())
}
