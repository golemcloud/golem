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

test_r::enable!();

#[test_r::sequential]
mod tests {
    use golem_client::api::RegistryServiceClient;
    use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
    use golem_common::{agent_id, data_value};
    use golem_test_framework::config::{
        EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    };
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};
    use std::sync::Once;
    use std::time::Duration;
    use test_r::{test, timeout};

    static TRACING_INIT: Once = Once::new();

    fn init_tracing() {
        TRACING_INIT.call_once(|| {
            init_tracing_with_default_debug_env_filter(
                &TracingConfig::test_pretty_without_time("memory-billing").with_env_overrides(),
            );
        });
    }

    async fn create_deps() -> EnvBasedTestDependencies {
        init_tracing();
        let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
            worker_executor_cluster_size: 1,
            ..EnvBasedTestDependenciesConfig::new()
        })
        .await
        .expect("Failed constructing memory-billing test dependencies");

        let cluster = deps.worker_executor_cluster();
        cluster.kill_all().await;
        cluster
            .restart_all_with_extra_env_vars(vec![
                (
                    "GOLEM__RESOURCE_LIMITS__CONFIG__BATCH_UPDATE_INTERVAL".to_string(),
                    "200ms".to_string(),
                ),
                (
                    "GOLEM__RESOURCE_LIMITS__CONFIG__LIMIT_REFRESH_INTERVAL".to_string(),
                    "1s".to_string(),
                ),
            ])
            .await;

        deps
    }

    async fn memory_gb_seconds(
        deps: &EnvBasedTestDependencies,
        user: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
    ) -> anyhow::Result<u64> {
        let usage = deps
            .registry_service()
            .client(&user.token)
            .await
            .get_account_storage_usage(&user.account_id.0, None)
            .await?;
        Ok(usage.usage.memory_gb_seconds)
    }

    async fn wait_for_memory_billing(
        deps: &EnvBasedTestDependencies,
        user: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
        must_exceed: u64,
    ) -> anyhow::Result<u64> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let current = memory_gb_seconds(deps, user).await?;
            if current > must_exceed {
                return Ok(current);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "linear memory billing remained at {current} GB-seconds; expected more than {must_exceed}"
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    #[test]
    #[timeout("2m")]
    async fn allocated_linear_memory_reaches_account_usage() -> anyhow::Result<()> {
        let deps = create_deps().await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "scalability_large_dynamic_memory_release")
            .name("scalability:large-dynamic-memory")
            .store()
            .await?;
        let agent = agent_id!("LargeDynamicMemoryAgent", "memory-billing");
        let worker = user.start_agent(&component.id, agent.clone()).await?;
        let before = memory_gb_seconds(&deps, &user).await?;

        user.invoke_and_await_agent(
            &component,
            &agent,
            "run_with_memory_and_work",
            data_value!(128u64, 10_000u64),
        )
        .await?;

        let after = wait_for_memory_billing(&deps, &user, before).await?;
        assert!(
            after > before,
            "real agent allocation should increase account memory usage"
        );
        user.delete_worker(&worker).await?;

        Ok(())
    }
}
