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
use golem_client::api::RegistryServiceClient;
use golem_common::model::quota::{
    EnforcementAction, ResourceCapacityLimit, ResourceDefinitionCreation, ResourceLimit,
    ResourceName, ResourceRateLimit, TimePeriod,
};
use golem_common::model::{AgentId, AgentStatus};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::task::JoinSet;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

use golem_common::model::quota::ResourceDefinition;

async fn provision_capacity_resource(
    client: &impl RegistryServiceClient,
    env_id: &uuid::Uuid,
    name: &str,
    capacity: u64,
    enforcement_action: EnforcementAction,
) -> anyhow::Result<ResourceDefinition> {
    let def = client
        .create_resource(
            env_id,
            &ResourceDefinitionCreation {
                name: ResourceName(name.to_string()),
                limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: capacity }),
                enforcement_action,
                unit: "unit".to_string(),
                units: "units".to_string(),
            },
        )
        .await?;
    Ok(def)
}

async fn provision_rate_resource(
    client: &impl RegistryServiceClient,
    env_id: &uuid::Uuid,
    name: &str,
    rate: u64,
    max_burst: u64,
    period: TimePeriod,
    enforcement_action: EnforcementAction,
) -> anyhow::Result<ResourceDefinition> {
    let def = client
        .create_resource(
            env_id,
            &ResourceDefinitionCreation {
                name: ResourceName(name.to_string()),
                limit: ResourceLimit::Rate(ResourceRateLimit {
                    value: rate,
                    period,
                    max: max_burst,
                }),
                enforcement_action,
                unit: "request".to_string(),
                units: "requests".to_string(),
            },
        )
        .await?;
    Ok(def)
}

/// Reserve `amount` units and commit the exact amount back.  The agent should
/// return the reserved and committed amounts as a record.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn reserve_and_commit_succeeds(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(
        &client,
        &env.id.0,
        "tokens",
        10_000,
        EnforcementAction::Throttle,
    )
    .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "reserve-and-commit-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "reserve_and_commit",
            data_value!("tokens".to_string(), 100u64, 50u64, 50u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::Record(vec![Value::U64(50), Value::U64(50)]));
    Ok(())
}

/// Reserve `amount` and commit less than reserved.  The unused capacity should
/// be returned to the pool (verified by a second reservation succeeding).
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn partial_commit_returns_unused_capacity(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(&client, &env.id.0, "tokens", 100, EnforcementAction::Reject)
        .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "partial-commit-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "reserve_and_commit",
            data_value!("tokens".to_string(), 100u64, 100u64, 20u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::Record(vec![Value::U64(100), Value::U64(20)]));

    let second = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "try_reserve",
            data_value!("tokens".to_string(), 100u64, 80u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(second, Value::Bool(true));
    Ok(())
}

/// Drop a reservation without calling commit. Dropping does not return any used capacity.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn drop_without_commit_is_zero_commit(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(&client, &env.id.0, "tokens", 100, EnforcementAction::Reject)
        .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "drop-no-commit-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let reserved = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "reserve_and_drop",
            data_value!("tokens".to_string(), 100u64, 80u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(reserved, Value::U64(80));

    let second = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "try_reserve",
            data_value!("tokens".to_string(), 100u64, 30u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(second, Value::Bool(false));

    Ok(())
}

/// When the enforcement policy is `Reject` and the resource has no remaining
/// capacity, `try_reserve` must return `false` without trapping.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn reject_policy_returns_failed_reservation(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;

    let client = deps.registry_service().client(&user.token).await;
    provision_capacity_resource(&client, &env.id.0, "limited", 0, EnforcementAction::Reject)
        .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "reject-policy-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "try_reserve",
            data_value!("limited".to_string(), 1u64, 1u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::Bool(false));
    Ok(())
}

/// Split a token into parent and child halves, reserve from each, and verify
/// both reservations succeed independently.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn split_tokens_are_independently_functional(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(
        &client,
        &env.id.0,
        "tokens",
        10_000,
        EnforcementAction::Throttle,
    )
    .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "split-and-reserve-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "split_and_reserve",
            data_value!("tokens".to_string(), 100u64, 40u64, 10u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::Record(vec![Value::U64(10), Value::U64(10)]));
    Ok(())
}

/// Merge two tokens for the same resource, then reserve from the merged token.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn merge_and_reserve_succeeds(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(
        &client,
        &env.id.0,
        "tokens",
        10_000,
        EnforcementAction::Throttle,
    )
    .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "merge-and-reserve-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "merge_and_reserve",
            data_value!("tokens".to_string(), 60u64, 40u64, 20u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::U64(20));
    Ok(())
}

/// Acquire a token and immediately drop it.  The lease reference count should
/// decrement without a panic and the agent should remain healthy.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn acquire_and_drop_does_not_panic(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(
        &client,
        &env.id.0,
        "tokens",
        110,
        EnforcementAction::Throttle,
    )
    .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "acquire-and-drop-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &agent_id,
        "acquire_and_drop",
        data_value!("tokens".to_string(), 100u64),
    )
    .await?;

    let result = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "try_reserve",
            data_value!("tokens".to_string(), 100u64, 10u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, Value::Bool(true));
    Ok(())
}

/// Two agents in the same environment compete for the same resource.  The
/// total successfully reserved units across both agents must not exceed the
/// provisioned capacity.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn shared_quota_across_two_agents(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(&client, &env.id.0, "shared", 10, EnforcementAction::Reject)
        .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_a = agent_id!("QuotaApi", "shared-quota-a");
    let agent_b = agent_id!("QuotaApi", "shared-quota-b");
    user.start_agent(&component.id, agent_a.clone()).await?;
    user.start_agent(&component.id, agent_b.clone()).await?;

    let mut tasks = JoinSet::new();
    for (agent_id, expected_use) in [(agent_a, 10u64), (agent_b, 20u64)] {
        let user = user.clone();
        let component = component.clone();
        tasks.spawn(async move {
            user.invoke_and_await_agent(
                &component,
                &agent_id,
                "try_reserve_and_commit",
                data_value!("shared".to_string(), expected_use, 8u64),
            )
            .await
        });
    }

    let mut total_reserved = 0u64;
    while let Some(res) = tasks.join_next().await {
        let ret = res??
            .into_return_value()
            .ok_or_else(|| anyhow!("expected return value"))?;
        if let Value::U64(n) = ret {
            total_reserved += n;
        } else {
            anyhow::bail!("unexpected return type: {ret:?}");
        }
    }

    assert_eq!(total_reserved, 8);

    Ok(())
}

/// Reserving from a zero-capacity Terminate resource should transition the
/// agent to Failed status
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn terminate_enforcement_fails_agent(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_capacity_resource(&client, &env.id.0, "fatal", 0, EnforcementAction::Terminate)
        .await?;

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let agent_id = agent_id!("QuotaApi", "terminate-enforcement-1");
    let agent_system_id = user.start_agent(&component.id, agent_id.clone()).await?;

    let _ = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "reserve_and_commit",
            data_value!("fatal".to_string(), 1u64, 1u64, 1u64),
        )
        .await;

    user.wait_for_status(
        &agent_system_id,
        AgentStatus::Failed,
        Duration::from_secs(30),
    )
    .await?;

    Ok(())
}

/// An agent loops making HTTP calls, reserving 1 unit per call against a
/// rate-limited resource with Reject enforcement. The loop stops when the
/// first rejection is returned. The HTTP server counts incoming requests.
/// The received count must equal the provisioned rate value.
#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn rate_limit_reject_stops_at_limit(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_rate_resource(
        &client,
        &env.id.0,
        "api-calls",
        5,
        5,
        TimePeriod::Minute,
        EnforcementAction::Reject,
    )
    .await?;

    let received = Arc::new(AtomicU64::new(0));
    let received_clone = received.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    let http_server = tokio::spawn(async move {
        use axum::{Router, routing::get};
        let route = Router::new().route(
            "/call",
            get(move || {
                let cnt = received_clone.clone();
                async move {
                    cnt.fetch_add(1, Ordering::Relaxed);
                    "ok"
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let host = "localhost".to_string();

    let agent_id = agent_id!("QuotaApi", "rate-reject-1");
    user.start_agent(&component.id, agent_id.clone()).await?;

    let calls_made = user
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "reserve_in_loop",
            data_value!(
                "api-calls".to_string(),
                10u64,
                host.clone(),
                port as u64,
                100u64
            ),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    http_server.abort();

    let server_count = received.load(Ordering::SeqCst);
    let Value::U64(agent_count) = calls_made else {
        anyhow::bail!("unexpected return: {calls_made:?}");
    };

    assert_eq!(agent_count, 5, "agent made {agent_count} calls, expected 5");
    assert_eq!(
        server_count, agent_count,
        "server saw {server_count} requests but agent reported {agent_count}"
    );

    Ok(())
}

/// Verifies that a quota token can be split and sent to a second Rust agent
/// over RPC. The sender creates a token, splits half the expected-use to a
/// `QuotaRpcReceiver` agent, and both agents make HTTP calls in parallel.
/// The combined call count must not exceed the burst capacity of the original
/// token.
#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn quota_token_rpc_rust(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_rate_resource(
        &client,
        &env.id.0,
        "rpc-shared-rate",
        4,
        4,
        TimePeriod::Hour,
        EnforcementAction::Throttle,
    )
    .await?;

    let received = Arc::new(AtomicU64::new(0));
    let received_clone = received.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    let http_server = tokio::spawn(async move {
        use axum::{Router, routing::get};
        let route = Router::new().route(
            "/call",
            get(move || {
                let cnt = received_clone.clone();
                async move {
                    cnt.fetch_add(1, Ordering::SeqCst);
                    "ok"
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let component = user
        .component(&env.id, "golem_it_agent_sdk_rust_release")
        .name("golem-it:agent-sdk-rust")
        .store()
        .await?;

    let sender = agent_id!("QuotaRpcSender", "rpc-sender");
    let sender_sys = user.start_agent(&component.id, sender.clone()).await?;

    user.invoke_agent(
        &component,
        &sender,
        "split_and_loop",
        data_value!(
            "rpc-shared-rate".to_string(),
            4u64,
            2u64,
            "localhost".to_string(),
            port as u64,
            4u64
        ),
    )
    .await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    user.wait_for_statuses(
        &sender_sys,
        &[AgentStatus::Idle, AgentStatus::Suspended],
        Duration::from_secs(60),
    )
    .await?;

    user.wait_for_statuses(
        &AgentId {
            component_id: component.id,
            agent_id: "QuotaRpcReceiver(\"rpc-sender-receiver\")".to_string(),
        },
        &[AgentStatus::Idle, AgentStatus::Suspended],
        Duration::from_secs(60),
    )
    .await?;

    http_server.abort();

    let total = received.load(Ordering::SeqCst);

    assert_eq!(total, 4);

    Ok(())
}

/// Same as `quota_token_rpc_rust` but using the TypeScript SDK test component.
#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn quota_token_rpc_ts(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_rate_resource(
        &client,
        &env.id.0,
        "rpc-shared-rate-ts",
        4,
        4,
        TimePeriod::Hour,
        EnforcementAction::Throttle,
    )
    .await?;

    let received = Arc::new(AtomicU64::new(0));
    let received_clone = received.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    let http_server = tokio::spawn(async move {
        use axum::{Router, routing::get};
        let route = Router::new().route(
            "/call",
            get(move || {
                let cnt = received_clone.clone();
                async move {
                    cnt.fetch_add(1, Ordering::SeqCst);
                    "ok"
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let component = user
        .component(&env.id, "golem_it_agent_sdk_ts")
        .name("golem-it:agent-sdk-ts")
        .store()
        .await?;

    let sender = agent_id!("QuotaRpcSender", "rpc-sender-ts");

    tracing::warn!("here0");
    let sender_sys = user.start_agent(&component.id, sender.clone()).await?;

    tracing::warn!("here1");
    user.invoke_agent(
        &component,
        &sender,
        "splitAndLoop",
        data_value!(
            "rpc-shared-rate-ts".to_string(),
            4u64,
            2u64,
            "localhost".to_string(),
            port as u64,
            4u64
        ),
    )
    .await?;

    tracing::warn!("here2");
    tokio::time::sleep(Duration::from_secs(1)).await;

    user.wait_for_statuses(
        &sender_sys,
        &[AgentStatus::Idle, AgentStatus::Suspended],
        Duration::from_secs(60),
    )
    .await?;

    user.wait_for_statuses(
        &AgentId {
            component_id: component.id,
            agent_id: "QuotaRpcReceiver(\"rpc-sender-ts-receiver\")".to_string(),
        },
        &[AgentStatus::Idle, AgentStatus::Suspended],
        Duration::from_secs(60),
    )
    .await?;

    http_server.abort();

    let total = received.load(Ordering::SeqCst);

    assert_eq!(total, 4);

    Ok(())
}

#[test_r::test]
async fn rate_limit_throttle_two_agents(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let client = deps.registry_service().client(&user.token).await;

    provision_rate_resource(
        &client,
        &env.id.0,
        "shared-rate",
        4,
        4,
        TimePeriod::Hour,
        EnforcementAction::Throttle,
    )
    .await?;

    let received = Arc::new(AtomicU64::new(0));
    let received_clone = received.clone();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    let http_server = tokio::spawn(async move {
        use axum::{Router, routing::get};
        let route = Router::new().route(
            "/call",
            get(move || {
                let cnt = received_clone.clone();
                async move {
                    cnt.fetch_add(1, Ordering::SeqCst);
                    "ok"
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .store()
        .await?;

    let host = "localhost".to_string();

    let agent_a = agent_id!("QuotaApi", "rate-throttle-a");
    let agent_b = agent_id!("QuotaApi", "rate-throttle-b");
    let agent_a_sys = user.start_agent(&component.id, agent_a.clone()).await?;
    let agent_b_sys = user.start_agent(&component.id, agent_b.clone()).await?;

    let mut tasks = JoinSet::new();
    for (agent_id, agent_sys_id) in [
        (agent_a.clone(), agent_a_sys.clone()),
        (agent_b.clone(), agent_b_sys.clone()),
    ] {
        let user = user.clone();
        let component = component.clone();
        let host = host.clone();
        tasks.spawn(async move {
            user.invoke_agent(
                &component,
                &agent_id,
                "reserve_in_loop",
                data_value!("shared-rate".to_string(), 10u64, host, port as u64, 4u64),
            )
            .await?;

            tokio::time::sleep(Duration::from_secs(1)).await;

            user.wait_for_statuses(
                &agent_sys_id,
                &[AgentStatus::Idle, AgentStatus::Suspended],
                Duration::from_secs(60),
            )
            .await
        });
    }

    while let Some(r) = tasks.join_next().await {
        r??;
    }

    http_server.abort();

    let total = received.load(Ordering::SeqCst);

    assert_eq!(total, 4);

    Ok(())
}
