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

//! The Go SDK's saga helpers: a committed transaction runs only its forward
//! steps, while a failing one rolls back — running the compensation of every step
//! that had already succeeded. The agent records each step and compensation as it
//! runs, so the recorded order is the assertion.

use crate::Tracing;
use golem_common::model::oplog::payload::{
    HostRequestGolemRetryNextDelay, HostResponseGolemRetryNextDelay,
};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::retry_policy::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};
use golem_common::schema::FromSchema;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::HashMap;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// All steps succeed: the transaction commits and no compensation runs.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_transaction_commits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("SagaAgent", "go-saga-ok");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let outcome = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!(false))
        .await?
        .into_typed::<String>()?;
    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "log", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(outcome, "committed");
    assert_eq!(recorded, vec!["charge".to_string(), "ship".to_string()]);
    Ok(())
}

/// A later step fails: the transaction rolls back, running the compensation of
/// the step that had already succeeded (charge → refund). The failed step is not
/// compensated, since it never took effect.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_transaction_rolls_back_with_compensation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("SagaAgent", "go-saga-fail");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let outcome = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!(true))
        .await?
        .into_typed::<String>()?;
    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "log", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(outcome, "rolled-back");
    assert_eq!(
        recorded,
        vec![
            "charge".to_string(),
            "ship".to_string(),
            "refund".to_string()
        ]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_infallible_saga_exhausts_after_durable_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    #[derive(Debug, FromSchema)]
    #[schema(rename_all = "camelCase")]
    struct RetryOutcome {
        attempts: u32,
        compensations: u32,
        exhausted: bool,
        rolled_back_fully: bool,
        policy_error: bool,
        error: String,
        elapsed_nanos: u64,
    }

    let context = TestContext::new(last_unique_id);
    let agent_id = agent_id!("SagaAgent", "go-saga-exhaustion");
    let delay = Duration::from_millis(50);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            retry_policies: Some(vec![
                NamedRetryPolicy {
                    name: "bounded-saga".to_string(),
                    priority: 100,
                    predicate: Predicate::And(
                        Box::new(Predicate::PropEq {
                            property: "verb".to_string(),
                            value: PredicateValue::Text("saga".to_string()),
                        }),
                        Box::new(Predicate::PropMatches {
                            property: "noun-uri".to_string(),
                            pattern: format!("urn:worker:*/{agent_id}"),
                        }),
                    ),
                    policy: RetryPolicy::CountBox {
                        max_retries: 2,
                        inner: Box::new(RetryPolicy::Periodic(delay)),
                    },
                },
                NamedRetryPolicy {
                    name: "unmatched-saga-must-not-retry".to_string(),
                    priority: 0,
                    predicate: Predicate::True,
                    policy: RetryPolicy::Never,
                },
            ]),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let outcome = executor
        .invoke_and_await_agent(&component, &agent_id, "exhaustRetries", data_value!())
        .await?
        .into_typed::<RetryOutcome>()?;
    assert_eq!(outcome.attempts, 3);
    assert_eq!(outcome.compensations, 3);
    assert!(outcome.exhausted && outcome.rolled_back_fully);
    assert!(!outcome.policy_error);
    assert_eq!(outcome.error, "retryable saga failure");
    assert!(outcome.elapsed_nanos >= 2 * delay.as_nanos() as u64);
    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "log", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(recorded, ["attempt", "compensate"].repeat(3));

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let decisions = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name == "golem::api::retry::next_retry_delay" =>
            {
                Some((entry.oplog_index, start))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(decisions.len(), 3);
    let mut state = Vec::new();
    for (attempt, (index, start)) in decisions.iter().enumerate() {
        let request = HostRequestGolemRetryNextDelay::from_value(
            start.request.as_ref().expect("retry request").value(),
        )?;
        assert_eq!(request.verb, "saga");
        assert_eq!(
            request.noun_uri,
            format!("urn:worker:{}/{agent_id}", component.id)
        );
        assert_eq!(request.state, state);
        let (end_index, end) = oplog
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::End(end) if end.start_index == *index => {
                    Some((entry.oplog_index, end))
                }
                _ => None,
            })
            .expect("durable retry decision completed");
        let response = HostResponseGolemRetryNextDelay::from_value(
            end.response.as_ref().expect("retry response").value(),
        )?;
        let (next_state, next_delay) = response.result.expect("valid retry policy");
        assert_eq!(next_delay, (attempt < 2).then_some(delay.as_nanos() as u64));
        state = next_state;

        if let Some((next_index, _)) = decisions.get(attempt + 1) {
            assert!(oplog.iter().any(|entry| {
                entry.oplog_index > end_index
                    && entry.oplog_index < *next_index
                    && matches!(&entry.entry, PublicOplogEntry::Start(wait)
                        if wait.function_name == "clocks::monotonic-clock::wait-for")
                    && oplog.iter().any(|terminal| {
                        terminal.oplog_index < *next_index
                            && matches!(&terminal.entry, PublicOplogEntry::End(end)
                                if end.start_index == entry.oplog_index)
                    })
                    && oplog.iter().any(|delivered| {
                        delivered.oplog_index < *next_index
                            && matches!(&delivered.entry, PublicOplogEntry::CompletionDelivered(done)
                                if done.start_index == entry.oplog_index)
                    })
            }), "retry must wait for a durable timer completion before the next attempt");
        }
    }
    Ok(())
}
