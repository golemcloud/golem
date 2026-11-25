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

use crate::Tracing;
use anyhow::anyhow;
use golem_client::api::{AgentClient, AgentError, InvocationClient, InvocationError};
use golem_client::model::{
    AgentInvocationMode, AgentInvocationRequest, InvocationKind, InvocationOutcome,
    InvocationState, InvocationSummary,
};
use golem_common::base_model::api::error_code;
use golem_common::model::{AgentStatus, IdempotencyKey, PromiseId};
use golem_common::schema::{ExternalSchemaValue, FromSchema, TypedSchemaValue};
use golem_common::{agent_key, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

fn error_code<T: std::fmt::Debug>(
    result: Result<T, golem_client::Error<InvocationError>>,
) -> (u16, String) {
    match result {
        Err(golem_client::Error::Item(error)) => match error {
            InvocationError::Error400(body) => (400, body.code),
            InvocationError::Error401(body) => (401, body.code),
            InvocationError::Error403(body) => (403, body.code),
            InvocationError::Error404(body) => (404, body.code),
            InvocationError::Error409(body) => (409, body.code),
            other => panic!("unexpected invocation error {other:?}"),
        },
        other => panic!("expected an invocation error, got {other:?}"),
    }
}

/// Exercises get, list, lookup and cancel of the environment-scoped invocation API on one agent
/// that holds a completed, a waiting and a pending invocation.
#[test]
#[tracing::instrument]
#[timeout("6m")]
async fn invocation_api_reads_and_cancels_invocations(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .store()
        .await?;
    let agent = agent_key!("RpcBlockingCounter", "rest-invocations");
    let agent_id = user.start_agent(&component.id, agent.clone()).await?;
    let agent_uuid = agent_id.agent_id.0;
    let env_id = env.id.0;
    let client = user
        .deps
        .worker_service()
        .invocation_http_client(&user.token)
        .await;
    let lookup_one = async |key: &IdempotencyKey| -> anyhow::Result<InvocationSummary> {
        let mut found = client
            .lookup_invocations(&env_id, &agent_uuid, None, &key.value)
            .await?
            .invocations;
        assert_eq!(found.len(), 1, "one invocation per key and agent");
        Ok(found.remove(0))
    };

    // A completed invocation shows its state, operation and decoded result.
    user.invoke_and_await_agent(&component, &agent, "inc_by", data_value!(5u64))
        .await?;
    let value_key = IdempotencyKey::fresh();
    user.invoke_and_await_agent_with_key(
        &component,
        &agent,
        &value_key,
        "get_value",
        data_value!(),
    )
    .await?;
    let completed = lookup_one(&value_key).await?;
    assert_eq!(completed.state, InvocationState::Completed);
    assert_eq!(completed.kind, Some(InvocationKind::Method));
    assert_eq!(completed.operation.as_deref(), Some("get_value"));
    assert_eq!(completed.environment_id, env_id);
    let got = client
        .get_invocation(&env_id, &completed.invocation_id)
        .await?;
    assert_eq!(got.summary.invocation_id, completed.invocation_id);
    let Some(InvocationOutcome::Method(method)) = got.result else {
        anyhow::bail!(
            "a completed method invocation has its result: {:?}",
            got.result
        );
    };
    let value = TypedSchemaValue::from(method.value);
    assert_eq!(
        <u64 as FromSchema>::from_value(value.value()).map_err(|e| anyhow!("{e}"))?,
        5
    );

    // A blocked invocation keeps the next one pending.
    let promise = user
        .invoke_and_await_agent(&component, &agent, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let waiting_key = IdempotencyKey::fresh();
    user.invoke_agent_with_key(
        &component,
        &agent,
        &waiting_key,
        "await_promise",
        data_value!(promise.clone()),
    )
    .await?;
    user.wait_for_status(&agent_id, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    let pending_key = IdempotencyKey::fresh();
    user.invoke_agent_with_key(
        &component,
        &agent,
        &pending_key,
        "inc_by",
        data_value!(1u64),
    )
    .await?;
    let waiting = lookup_one(&waiting_key).await?;
    assert!(
        matches!(
            waiting.state,
            InvocationState::Running | InvocationState::Waiting
        ),
        "{waiting:?}"
    );
    let pending = lookup_one(&pending_key).await?;
    assert_eq!(pending.state, InvocationState::Pending);
    assert_eq!(pending.operation.as_deref(), Some("inc_by"));

    // Paging returns every invocation exactly once: the constructor run, inc_by, get_value,
    // create_promise, await_promise and the pending inc_by.
    let mut listed = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = client
            .list_invocations(
                &env_id,
                &agent_uuid,
                None,
                None,
                None,
                None,
                None,
                Some(2),
                cursor.as_deref(),
            )
            .await?;
        assert!(page.invocations.len() <= 2);
        listed.extend(page.invocations);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let listed_ids = listed
        .iter()
        .map(|invocation| invocation.invocation_id.clone())
        .collect::<HashSet<_>>();
    assert_eq!(listed.len(), 6, "{listed:?}");
    assert_eq!(listed_ids.len(), 6);
    for expected in [&completed, &waiting, &pending] {
        assert!(listed_ids.contains(&expected.invocation_id));
    }
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0].changed_at >= pair[1].changed_at),
        "newest change first"
    );

    // Filters.
    let only_pending = client
        .list_invocations(
            &env_id,
            &agent_uuid,
            None,
            Some(&[InvocationState::Pending]),
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert_eq!(
        only_pending
            .invocations
            .iter()
            .map(|invocation| invocation.invocation_id.clone())
            .collect::<Vec<_>>(),
        vec![pending.invocation_id.clone()]
    );
    let constructor = client
        .list_invocations(
            &env_id,
            &agent_uuid,
            None,
            None,
            Some(&InvocationKind::Constructor),
            None,
            None,
            None,
            None,
        )
        .await?;
    assert_eq!(constructor.invocations.len(), 1);
    assert_eq!(constructor.invocations[0].operation, None);
    let inc_by = client
        .list_invocations(
            &env_id,
            &agent_uuid,
            Some("inc_by"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;
    assert_eq!(inc_by.invocations.len(), 2);
    // `since` and `until` are inclusive bounds on the last change.
    let bound = completed.changed_at;
    for (since, until) in [
        (Some(bound), None),
        (None, Some(bound)),
        (Some(bound), Some(bound)),
    ] {
        let page = client
            .list_invocations(
                &env_id,
                &agent_uuid,
                None,
                None,
                None,
                since.as_ref(),
                until.as_ref(),
                None,
                None,
            )
            .await?;
        let expected = listed
            .iter()
            .filter(|invocation| {
                since.is_none_or(|since| invocation.changed_at >= since)
                    && until.is_none_or(|until| invocation.changed_at <= until)
            })
            .map(|invocation| invocation.invocation_id.clone())
            .collect::<HashSet<_>>();
        assert!(expected.contains(&completed.invocation_id));
        assert_eq!(
            page.invocations
                .iter()
                .map(|invocation| invocation.invocation_id.clone())
                .collect::<HashSet<_>>(),
            expected,
            "since {since:?}, until {until:?}"
        );
    }
    assert_eq!(
        error_code(
            client
                .list_invocations(
                    &env_id,
                    &agent_uuid,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some("not-a-cursor"),
                )
                .await
        )
        .0,
        400
    );

    // An awaited call whose pending invocation is cancelled ends with 409 invocation-cancelled.
    let awaited_key = IdempotencyKey::fresh();
    let agent_client = user
        .deps
        .worker_service()
        .agent_http_client(&user.token)
        .await;
    let request = AgentInvocationRequest {
        agent_id: Some(agent_uuid),
        parameters: None,
        method: "inc_by".to_string(),
        method_parameters: ExternalSchemaValue::try_from(data_value!(10u64).into_parts().1)
            .map_err(anyhow::Error::msg)?,
        mode: AgentInvocationMode::Await,
        schedule_at: None,
        expected_fingerprint: None,
    };
    let agent_type_name = agent.agent_type.0.clone();
    let awaited = {
        let awaited_key = awaited_key.value.clone();
        tokio::spawn(async move {
            agent_client
                .invoke_agent(&env_id, &agent_type_name, Some(&awaited_key), &request)
                .await
                .map(|_| ())
        })
    };
    let awaited_pending = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let found = client
                .lookup_invocations(&env_id, &agent_uuid, None, &awaited_key.value)
                .await;
            if let Ok(mut found) = found {
                break found.invocations.remove(0);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    assert_eq!(awaited_pending.state, InvocationState::Pending);
    let cancelled = client
        .cancel_invocation(&env_id, &awaited_pending.invocation_id)
        .await?;
    assert_eq!(cancelled.state, InvocationState::Cancelled);
    match awaited.await? {
        Err(golem_client::Error::Item(AgentError::Error409(body))) => {
            assert_eq!(body.code, error_code::INVOCATION_CANCELLED);
        }
        other => anyhow::bail!("expected 409 invocation-cancelled, got {other:?}"),
    }

    // Cancelling a pending invocation makes it cancelled; cancelling it again keeps the state.
    let cancelled = client
        .cancel_invocation(&env_id, &pending.invocation_id)
        .await?;
    assert_eq!(cancelled.state, InvocationState::Cancelled);
    assert_eq!(
        client
            .get_invocation(&env_id, &pending.invocation_id)
            .await?
            .summary
            .state,
        InvocationState::Cancelled
    );
    assert_eq!(
        client
            .cancel_invocation(&env_id, &pending.invocation_id)
            .await?
            .state,
        InvocationState::Cancelled
    );
    assert_eq!(
        client
            .cancel_invocation(&env_id, &completed.invocation_id)
            .await?
            .state,
        InvocationState::Completed
    );

    // A started invocation cannot be cancelled yet.
    assert_eq!(
        error_code(
            client
                .cancel_invocation(&env_id, &waiting.invocation_id)
                .await
        ),
        (
            409,
            error_code::CANNOT_CANCEL_RUNNING_INVOCATION.to_string()
        )
    );

    // The ID resolves only in its agent's environment, and unknown IDs are not found.
    let (_, other_env) = user.app_and_env().await?;
    assert_eq!(
        error_code(
            client
                .get_invocation(&other_env.id.0, &waiting.invocation_id)
                .await
        )
        .0,
        404
    );
    // A caller cannot read agents in an environment it cannot view, including a missing one.
    assert_eq!(
        error_code(
            client
                .get_invocation(&Uuid::new_v4(), &waiting.invocation_id)
                .await
        )
        .0,
        403
    );
    assert_eq!(
        error_code(
            client
                .get_invocation(
                    &env_id,
                    &format!("{}.{}", agent_id.agent_id, Uuid::new_v4())
                )
                .await
        ),
        (404, error_code::INVOCATION_NOT_FOUND.to_string())
    );
    assert_eq!(
        error_code(
            client
                .lookup_invocations(&env_id, &agent_uuid, None, "unused-key")
                .await
        ),
        (404, error_code::INVOCATION_NOT_FOUND.to_string())
    );
    assert_eq!(
        error_code(client.get_invocation(&env_id, "not-an-invocation-id").await).0,
        400
    );

    // Another account without a grant on the environment gets the answer of an unknown agent
    // (5.3 Rule 21), so it cannot tell that the invocation's agent exists.
    let other = deps.user().await?;
    let other_client = other
        .deps
        .worker_service()
        .invocation_http_client(&other.token)
        .await;
    assert_eq!(
        error_code(
            other_client
                .get_invocation(&env_id, &waiting.invocation_id)
                .await
        ),
        (404, error_code::AGENT_NOT_FOUND.to_string())
    );

    user.complete_promise(&env.id, &promise, Vec::new()).await?;
    Ok(())
}

/// The invoke endpoint returns `Golem-Invocation-Id` and `Golem-Environment-Id`, and the
/// invocation ID resolves as a percent-encoded path segment. The removed `DELETE` route is gone.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn invoke_returns_invocation_headers_that_resolve(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .store()
        .await?;
    let agent = agent_key!("RpcBlockingCounter", "headers \"a/b\" 雪");
    let agent_id = user.start_agent(&component.id, agent.clone()).await?;

    let worker_service = user.deps.worker_service();
    let base = format!(
        "http://{}:{}",
        worker_service.http_host(),
        worker_service.http_port()
    );
    let http = worker_service.base_http_client().await;
    let request = serde_json::json!({
        "agentId": agent_id.agent_id,
        "method": "inc_by",
        "methodParameters": ExternalSchemaValue::try_from(data_value!(3u64).into_parts().1)
            .map_err(anyhow::Error::msg)?,
        "mode": "await",
    });
    let response = http
        .post(format!(
            "{base}/v1/runtime/envs/{}/agent-types/{}/invoke",
            env.id.0, agent.agent_type.0
        ))
        .bearer_auth(user.token.secret())
        .json(&request)
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 200);
    let header = |name: &str| -> anyhow::Result<String> {
        Ok(response
            .headers()
            .get(name)
            .ok_or_else(|| anyhow!("missing {name}"))?
            .to_str()?
            .to_string())
    };
    let invocation_id = header("Golem-Invocation-Id")?;
    assert_eq!(header("Golem-Environment-Id")?, env.id.0.to_string());
    assert!(invocation_id.starts_with(&format!("{}.", agent_id.agent_id)));

    let got = http
        .get(format!(
            "{base}/v1/runtime/envs/{}/invocations/{invocation_id}",
            env.id.0
        ))
        .bearer_auth(user.token.secret())
        .send()
        .await?;
    assert_eq!(got.status().as_u16(), 200);
    let got: serde_json::Value = got.json().await?;
    assert_eq!(got["summary"]["invocationId"], invocation_id);
    assert_eq!(got["summary"]["state"], "completed");

    let removed = http
        .delete(format!(
            "{base}/v1/runtime/envs/{}/agents/{}/invocations/{}",
            env.id.0,
            agent_id.agent_id,
            Uuid::new_v4()
        ))
        .bearer_auth(user.token.secret())
        .send()
        .await?;
    assert!(
        matches!(removed.status().as_u16(), 404 | 405),
        "the cancel-by-key route is removed: {}",
        removed.status()
    );
    Ok(())
}

/// An invocation whose background work fails the agent after the result keeps the result. A
/// same-key REST invoke answers 200 with the recorded result and `backgroundFailure`, and the
/// invocation API shows `background-failed` with both the result and the error.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn a_background_failure_keeps_the_recorded_result(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_sdk_rust_release")
        .store()
        .await?;
    let agent = agent_key!("BackgroundWork", "rest-background-failure");
    let agent_id = user.start_agent(&component.id, agent.clone()).await?;
    let env_id = env.id.0;
    let client = user
        .deps
        .worker_service()
        .invocation_http_client(&user.token)
        .await;

    let release = user
        .invoke_and_await_agent(&component, &agent, "create_release", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let first = user
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &key,
            "trap_after_release",
            data_value!(release.clone()),
        )
        .await?;
    assert_eq!(first.background_failure(), None);
    assert_eq!(first.into_typed::<String>()?, "returned");
    user.complete_promise(&env.id, &release, vec![1]).await?;
    user.wait_for_status(&agent_id, AgentStatus::Failed, Duration::from_secs(60))
        .await?;

    let again = user
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &key,
            "trap_after_release",
            data_value!(release.clone()),
        )
        .await?;
    let failure = again.background_failure().map(str::to_string);
    assert!(
        failure
            .as_deref()
            .is_some_and(|failure| failure.contains("background work trapped after the result")),
        "the same key reports the background failure: {failure:?}"
    );
    assert_eq!(again.into_typed::<String>()?, "returned");

    let mut found = client
        .lookup_invocations(&env_id, &agent_id.agent_id.0, None, &key.value)
        .await?
        .invocations;
    assert_eq!(found.len(), 1, "one invocation per key and agent");
    let summary = found.remove(0);
    assert_eq!(summary.state, InvocationState::BackgroundFailed);
    let got = client
        .get_invocation(&env_id, &summary.invocation_id)
        .await?;
    assert_eq!(got.summary.state, InvocationState::BackgroundFailed);
    let Some(InvocationOutcome::BackgroundFailure(outcome)) = got.result else {
        anyhow::bail!(
            "a background-failed invocation has its result and its error: {:?}",
            got.result
        );
    };
    let value = TypedSchemaValue::from(
        outcome
            .value
            .ok_or_else(|| anyhow!("the background failure keeps the method result"))?,
    );
    assert_eq!(
        <String as FromSchema>::from_value(value.value()).map_err(|e| anyhow!("{e}"))?,
        "returned"
    );
    assert!(
        outcome
            .error
            .contains("background work trapped after the result"),
        "the outcome names the background failure: {}",
        outcome.error
    );
    Ok(())
}
