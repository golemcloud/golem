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
                &TracingConfig::test_pretty_without_time("storage-billing").with_env_overrides(),
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
        .expect("Failed constructing storage-billing test dependencies");

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
                    "GOLEM__FILESYSTEM_STORAGE__TOTAL_WORKER_FILESYSTEM_STORAGE_BYTES".to_string(),
                    (2 * 1024 * 1024).to_string(),
                ),
                (
                    "GOLEM__SUSPEND__SUSPEND_AFTER".to_string(),
                    "1s".to_string(),
                ),
            ])
            .await;

        deps
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

    async fn wait_for_durable_billing_to_settle(
        deps: &EnvBasedTestDependencies,
        user: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
        must_exceed: Option<f64>,
    ) -> anyhow::Result<f64> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut last = durable_byte_seconds(deps, user).await?;
        let mut unchanged_since = tokio::time::Instant::now();

        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let current = durable_byte_seconds(deps, user).await?;
            if current != last {
                last = current;
                unchanged_since = tokio::time::Instant::now();
            }
            if unchanged_since.elapsed() >= Duration::from_secs(1)
                && must_exceed.is_none_or(|minimum| current > minimum)
            {
                return Ok(current);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("durable storage billing did not settle before timeout");
            }
        }
    }

    fn assert_billing_window(actual: f64, min: f64, max: f64, phase: &str) {
        assert!(
            (min..=max).contains(&actual),
            "unexpected storage billing during {phase}: expected {min}..={max} byte-seconds, got {actual}"
        );
    }

    #[test]
    #[timeout("2m")]
    async fn provisioned_read_write_file_is_metered() -> anyhow::Result<()> {
        let deps = create_deps().await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "it_initial_file_system_release")
            .name("golem-it:initial-file-system")
            .unique()
            .with_files(
                "FileReadWrite",
                &[IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                }],
            )
            .store()
            .await?;

        let agent = agent_id!("FileReadWrite", "provisioned-storage-billing");
        let worker = user.start_agent(&component.id, agent.clone()).await?;
        let before = wait_for_durable_billing_to_settle(&deps, &user, None).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        user.delete_worker(&worker).await?;
        let after = wait_for_durable_billing_to_settle(&deps, &user, None).await?;

        assert_billing_window(after - before, 0.0, 1.0, "loaded-idle provisioned file");
        Ok(())
    }

    #[test]
    #[timeout("2m")]
    async fn permit_ownership_defines_storage_billing_window() -> anyhow::Result<()> {
        let deps = create_deps().await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "golem_it_host_api_tests_release")
            .store()
            .await?;
        let agent = agent_id!("FileSystem", "storage-billing-window");
        let worker = user.start_agent(&component.id, agent.clone()).await?;
        let contents = "x".repeat(1024 * 1024);
        user.invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/metered.txt", contents),
        )
        .await?;

        let idle_start = wait_for_durable_billing_to_settle(&deps, &user, None).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let idle_end = durable_byte_seconds(&deps, &user).await?;
        assert_billing_window(idle_end - idle_start, 0.0, 1.0, "loaded-idle interval");

        let active_start = idle_end;
        user.invoke_and_await_agent(&component, &agent, "sleep_for", data_value!(0.5f64))
            .await?;
        let active_end =
            wait_for_durable_billing_to_settle(&deps, &user, Some(active_start + 256.0 * 1024.0))
                .await?;
        assert_billing_window(
            active_end - active_start,
            256.0 * 1024.0,
            1024.0 * 1024.0,
            "permit-retaining host sleep",
        );

        let suspended_start = active_end;
        user.invoke_and_await_agent(&component, &agent, "sleep_for", data_value!(3.0f64))
            .await?;
        let suspended_end = wait_for_durable_billing_to_settle(&deps, &user, None).await?;
        // The one-second suspend threshold remains permit-owning; the remaining sleep is
        // suspended. Billing the full three seconds would exceed this bound by roughly 2x.
        assert_billing_window(
            suspended_end - suspended_start,
            0.0,
            1.5 * 1024.0 * 1024.0,
            "durable suspended interval",
        );

        user.delete_worker(&worker).await?;

        Ok(())
    }
}
