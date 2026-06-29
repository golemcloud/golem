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
use golem_common::model::{AgentStatus, RetryConfig};
use golem_common::{data_value, phantom_agent_id};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

async fn start_always_failing_http_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(
        async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                drop(stream);
            }
        }
        .in_current_span(),
    );

    port
}

fn delayed_recovery_retry_overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 100,
                min_delay: Duration::from_secs(30),
                max_delay: Duration::from_secs(30),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_millis(1);
        })),
        ..Default::default()
    }
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn interrupt_worker_during_delayed_recovery_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, delayed_recovery_retry_overrides()).await?;
    let port = start_always_failing_http_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = phantom_agent_id!("HttpClient", uuid::Uuid::new_v4());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "run", data_value!())
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Retrying, Duration::from_secs(20))
        .await?;

    tokio::time::timeout(Duration::from_secs(5), executor.interrupt(&worker_id)).await??;

    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, Duration::from_secs(5))
        .await?;

    let result = tokio::time::timeout(Duration::from_secs(5), invocation).await??;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Interrupted via the Golem API")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("90s")]
async fn delete_worker_during_delayed_recovery_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, delayed_recovery_retry_overrides()).await?;
    let port = start_always_failing_http_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = phantom_agent_id!("HttpClient", uuid::Uuid::new_v4());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let executor_clone = executor.clone();
    let component_clone = component.clone();
    let agent_id_clone = agent_id.clone();
    let invocation = tokio::spawn(
        async move {
            executor_clone
                .invoke_and_await_agent(&component_clone, &agent_id_clone, "run", data_value!())
                .await
        }
        .in_current_span(),
    );

    executor
        .wait_for_status(&worker_id, AgentStatus::Retrying, Duration::from_secs(20))
        .await?;

    tokio::time::timeout(Duration::from_secs(5), executor.delete_worker(&worker_id)).await??;

    let metadata = executor.get_worker_metadata(&worker_id).await;
    assert!(metadata.is_err());

    let _ = tokio::time::timeout(Duration::from_secs(5), invocation).await??;

    Ok(())
}
