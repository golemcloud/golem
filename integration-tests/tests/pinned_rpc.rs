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

use anyhow::{Context, ensure};
use golem_common::model::agent::{AgentTypeName, OwnedAgentId};
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_common::{agent_key, data_value};
use golem_test_framework::config::{
    DbType, EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    WorkerExecutorClusterControlDispatch, WorkerExecutorClusterControlStub,
};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use integration_tests::agents::{agent_type_id, keyed_agent_id};
use std::time::Duration;
use test_r::{test, test_dep, timeout};

test_r::enable!();

#[test_dep(scope = Hosted, worker = both(WorkerExecutorClusterControl))]
async fn dependencies() -> EnvBasedTestDependencies {
    init_tracing_with_default_debug_env_filter(
        &TracingConfig::test_pretty_without_time("pinned-rpc").with_env_overrides(),
    );
    EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 2,
        db_type: DbType::Sqlite,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("pinned RPC dependencies")
}

async fn assert_remote(
    deps: &EnvBasedTestDependencies,
    caller: &OwnedAgentId,
    child: &OwnedAgentId,
) -> anyhow::Result<()> {
    let routing = deps.shard_manager().get_routing_table().await?;
    ensure!(routing.all().len() == 2, "expected two executor owners");
    let caller_pod = routing.lookup(&caller.agent_id).context("caller owner")?;
    let child_pod = routing.lookup(&child.agent_id).context("child owner")?;
    ensure!(
        caller_pod != child_pod,
        "the kept handle must cross executors"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
async fn kept_rpc_handle_never_reaches_a_deleted_or_recreated_remote_child(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let caller_component = user
        .component(&env.id, "golem_it_host_api_tests_release")
        .name("golem-it:host-api-tests")
        .unique()
        .store()
        .await?;
    let child_component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;
    let caller_type = agent_type_id(
        &user,
        &caller_component,
        &AgentTypeName("GolemHostApi".into()),
    )
    .await?;
    let child_type = agent_type_id(
        &user,
        &child_component,
        &AgentTypeName("StreamingRpcTarget".into()),
    )
    .await?;
    let routing = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let routing = deps.shard_manager().get_routing_table().await?;
            if routing.all().len() == 2 {
                return Ok::<_, anyhow::Error>(routing);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await??;

    for recreate in [false, true] {
        let (caller, child, child_name) = (0..10_000)
            .find_map(|index| {
                let caller = agent_key!("GolemHostApi", format!("kept-remote-{recreate}-{index}"));
                let child_name = format!("kept-remote-child-{recreate}-{index}");
                let child = agent_key!("StreamingRpcTarget", child_name.clone());
                let caller_id = keyed_agent_id(&caller_component, caller_type, &caller).ok()?;
                let child_id = keyed_agent_id(&child_component, child_type, &child).ok()?;
                (routing.lookup(&caller_id.agent_id)? != routing.lookup(&child_id.agent_id)?)
                    .then_some((caller, child, child_name))
            })
            .context("no caller and child on different executors")?;
        let caller_id = user
            .start_agent(&caller_component.id, caller.clone())
            .await?;
        let child_id = user.start_agent(&child_component.id, child.clone()).await?;
        assert_remote(deps, &caller_id, &child_id).await?;

        let kept = user
            .invoke_and_await_agent(
                &caller_component,
                &caller,
                "keep_agent_rpc",
                data_value!("StreamingRpcTarget", child_name, "increment_scalar"),
            )
            .await?
            .into_typed::<Result<(), String>>()?;
        assert_eq!(kept, Ok(()));
        let stream = user
            .invoke_and_await_agent(
                &caller_component,
                &caller,
                "kept_agent_rpc_invoke_and_await_u64_stream_result",
                data_value!("increment_stream"),
            )
            .await?
            .into_typed::<Result<Vec<u64>, String>>()?;
        assert_eq!(stream, Ok(vec![2]));
        let original = user.get_worker_metadata(&child_id).await?.fingerprint;
        user.delete_worker(&child_id).await?;
        if recreate {
            let replacement = user.start_agent(&child_component.id, child.clone()).await?;
            assert_eq!(replacement, child_id);
            assert_ne!(
                user.get_worker_metadata(&child_id).await?.fingerprint,
                original
            );
            let fresh = user
                .invoke_and_await_agent(&child_component, &child, "increment_scalar", data_value!())
                .await?
                .into_typed::<u64>()?;
            assert_eq!(fresh, 1);
        }

        for method in [
            "kept_agent_rpc_invoke_and_await_result",
            "kept_agent_rpc_async_invoke_and_await_result",
            "kept_agent_rpc_invoke_result",
            "kept_agent_rpc_invoke_and_await_u64_stream_result",
        ] {
            assert_remote(deps, &caller_id, &child_id).await?;
            let streaming = method == "kept_agent_rpc_invoke_and_await_u64_stream_result";
            let result = user
                .invoke_and_await_agent(
                    &caller_component,
                    &caller,
                    method,
                    data_value!(if streaming {
                        "increment_stream"
                    } else {
                        "increment_scalar"
                    }),
                )
                .await?;
            let error = if streaming {
                result
                    .into_typed::<Result<Vec<u64>, String>>()?
                    .expect_err("stale stream handle")
            } else {
                result
                    .into_typed::<Result<(), String>>()?
                    .expect_err("stale scalar handle")
            };
            assert!(
                error.starts_with("RpcError::NotFound("),
                "{method}, recreate={recreate}: {error}"
            );
            assert_remote(deps, &caller_id, &child_id).await?;
            if recreate {
                let value = user
                    .invoke_and_await_agent(&child_component, &child, "scalar_value", data_value!())
                    .await?
                    .into_typed::<u64>()?;
                assert_eq!(value, 1, "{method} must not reach the replacement");
            } else {
                assert!(
                    user.get_worker_metadata_opt(&child_id).await?.is_none(),
                    "{method} must not recreate the child"
                );
            }
        }
        if recreate {
            let fresh = user
                .invoke_and_await_agent(&child_component, &child, "increment_scalar", data_value!())
                .await?
                .into_typed::<u64>()?;
            assert_eq!(fresh, 2);
        }
    }
    Ok(())
}
