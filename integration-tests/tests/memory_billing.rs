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
    use golem_common::model::AgentStatus;
    use golem_common::model::account::{AccountRevision, AccountSetPlan};
    use golem_common::model::account_usage::BYTE_SECONDS_PER_GB_MONTH;
    use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
    use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
    use golem_common::{agent_id, data_value};
    use golem_test_framework::config::{
        EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    };
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};
    use golem_test_framework::model::IFSEntry;
    use std::path::PathBuf;
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
                (
                    "GOLEM__RESOURCE_USAGE_METERING__MEMORY".to_string(),
                    "true".to_string(),
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

    async fn durable_byte_seconds(
        deps: &EnvBasedTestDependencies,
        user: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
    ) -> anyhow::Result<f64> {
        let usage = deps
            .registry_service()
            .client(&user.token)
            .await
            .get_account_storage_usage(&user.account_id.0, None)
            .await?;
        Ok(usage.usage.durable_storage_gb_month * BYTE_SECONDS_PER_GB_MONTH)
    }

    #[test]
    #[timeout("2m")]
    async fn permit_ownership_defines_allocated_memory_billing_window() -> anyhow::Result<()> {
        let deps = create_deps().await;
        let user = deps.user().await?;
        let admin = deps.admin().await;
        let admin_client = admin.registry_service_client().await;
        let unlimited_plan = admin_client
            .get_account_plan(&admin.account_id.0)
            .await?
            .plan_id;
        admin_client
            .set_account_plan(
                &user.account_id.0,
                &AccountSetPlan {
                    current_revision: AccountRevision::INITIAL,
                    plan: unlimited_plan,
                },
            )
            .await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "scalability_large_dynamic_memory_release")
            .name("scalability:large-dynamic-memory")
            .with_files(
                "LargeDynamicMemoryAgent",
                &[IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                }],
            )
            .store()
            .await?;
        let agent = agent_id!("LargeDynamicMemoryAgent", "memory-billing");
        let before = memory_gb_seconds(&deps, &user).await?;
        let durable_before = durable_byte_seconds(&deps, &user).await?;
        let worker = user.start_agent(&component.id, agent.clone()).await?;

        user.invoke_and_await_agent(&component, &agent, "run_with_delay", data_value!(3_000u64))
            .await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let after_host_wait = memory_gb_seconds(&deps, &user).await?;
        assert!(
            after_host_wait.saturating_sub(before) >= 1,
            "allocated memory must accrue while a permit-owning invocation waits in a host sleep: before={before}, after={after_host_wait}"
        );

        let invocation = user.invoke_and_await_agent(
            &component,
            &agent,
            "run_with_memory_and_work",
            data_value!(512u64, 5_000u64),
        );
        let recovery = async {
            user.wait_for_status(&worker, AgentStatus::Running, Duration::from_secs(10))
                .await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let before_replay = memory_gb_seconds(&deps, &user).await?;
            user.simulated_crash(&worker).await?;
            Ok::<u64, anyhow::Error>(before_replay)
        };
        let (invocation_result, before_replay) = tokio::join!(invocation, recovery);
        invocation_result?;
        let before_replay = before_replay?;

        tokio::time::sleep(Duration::from_secs(1)).await;
        let after_replay = memory_gb_seconds(&deps, &user).await?;
        assert!(
            after_replay.saturating_sub(before_replay) >= 2,
            "recovery of the interrupted 512 MiB workload must accrue memory after the pre-crash baseline: before={before_replay}, after={after_replay}"
        );

        user.wait_for_status(&worker, AgentStatus::Idle, Duration::from_secs(10))
            .await?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let after_permit_release = memory_gb_seconds(&deps, &user).await?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let after_idle = memory_gb_seconds(&deps, &user).await?;
        assert_eq!(
            after_idle, after_permit_release,
            "loaded-idle time after permit release must not accrue memory usage"
        );
        let durable_after = durable_byte_seconds(&deps, &user).await?;
        assert_eq!(
            durable_after, durable_before,
            "unmanaged read-write initial files must not accrue durable storage while memory billing is active"
        );

        user.delete_worker(&worker).await?;

        Ok(())
    }
}
